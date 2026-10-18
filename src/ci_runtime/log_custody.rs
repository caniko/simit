//! Qualification-consumer policy, separate from installed completion observers.
//! A native ZIP inventory is integrity evidence; it is never authority or completeness.
use super::log_archive::{self, ArchiveState, Budget, Comparison, Inventory};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

pub const SCHEMA: &str = "simit.native-log-reconciliation.v1";
const MAX_JSON: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Contract {
    #[default]
    StrictFreshV1,
    RetainedNativeV1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Verified,
    Failed,
    Unverified,
}

/// Exact immutable producer and target identity. No cross-run discovery is admitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub repository: String,
    pub repository_id: u64,
    pub run: u64,
    pub attempt: u64,
    pub source: String,
    pub event: String,
    pub branch: String,
    pub workflow: String,
    pub workflow_id: u64,
    pub workflow_revision: String,
    pub workflow_blob: String,
    pub pull_request: Option<Target>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub number: u64,
    pub base: String,
    pub base_ref: String,
}
impl Identity {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.repository.len() <= 256
                && self.repository.split('/').count() == 2
                && self
                    .repository
                    .split('/')
                    .all(|p| !p.is_empty() && p != "." && p != "..")
                && self
                    .repository
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b)),
            "invalid repository"
        );
        ensure!(
            self.repository_id > 0 && self.run > 0 && self.attempt == 1 && self.workflow_id > 0,
            "original attempt identity differs"
        );
        for sha in [&self.source, &self.workflow_revision, &self.workflow_blob] {
            ensure!(
                sha.len() == 40
                    && sha
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid immutable revision"
            );
        }
        ensure!(
            !self.branch.is_empty()
                && self.branch.len() <= 256
                && !self.branch.chars().any(char::is_control),
            "invalid branch"
        );
        ensure!(
            self.workflow.starts_with(".github/workflows/")
                && !self.workflow.contains('@')
                && self.workflow.len() < 256
                && self
                    .workflow
                    .split('/')
                    .all(|p| !p.is_empty() && p != "." && p != "..")
                && self
                    .workflow
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
                && (self.workflow.ends_with(".yml") || self.workflow.ends_with(".yaml")),
            "invalid enrolled workflow"
        );
        match (&*self.event, &self.pull_request) {
            ("push", None) => ensure!(
                self.workflow_revision == self.source,
                "push workflow revision differs from source"
            ),
            ("pull_request", Some(target)) => ensure!(
                target.number > 0
                    && target.base.len() == 40
                    && target
                        .base
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                    && !target.base_ref.is_empty()
                    && target.base_ref.len() <= 256
                    && !target.base_ref.chars().any(char::is_control),
                "invalid PR base"
            ),
            _ => anyhow::bail!("wrong event/target shape"),
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NonEmitting {
    pub job: u64,
    pub number: u64,
    pub name: String,
    pub reason: String,
    pub evidence_sha256: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Coverage {
    pub required_job_members: Vec<String>,
    pub missing_job_members: Vec<String>,
    pub ambiguous_jobs: Vec<String>,
    pub required_members: Vec<String>,
    pub missing_members: Vec<String>,
    pub skipped_steps: Vec<String>,
    pub non_emitting_steps: Vec<NonEmitting>,
    pub ambiguous_steps: Vec<String>,
}

/// Conservative provider naming subset. Unsupported transformations remain unverified.
pub fn coverage(
    jobs: &Value,
    inventory: &Inventory,
    exceptions: &[NonEmitting],
) -> Result<Coverage> {
    let rows = jobs["jobs"]
        .as_array()
        .context("missing attempt-specific jobs")?;
    ensure!(
        !rows.is_empty()
            && rows.len() <= 100
            && jobs["total_count"].as_u64() == Some(rows.len() as u64),
        "job inventory incomplete"
    );
    let mut result = Coverage::default();
    let mut job_ids = BTreeSet::new();
    let mut job_names = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut used = BTreeSet::new();
    for job in rows {
        let id = job["id"].as_u64().context("missing job id")?;
        let name = job["name"].as_str().context("missing job name")?;
        ensure!(
            id > 0 && job_ids.insert(id) && job_names.insert(name) && job["status"] == "completed",
            "ambiguous or incomplete job"
        );
        ensure!(
            !name.is_empty() && !name.contains(['/', '\\']) && !name.chars().any(char::is_control),
            "unsupported job naming"
        );
        let steps = job["steps"].as_array().context("missing steps")?;
        ensure!(steps.len() <= 10000, "step inventory exceeds member bound");
        if job["conclusion"] != "skipped" {
            let system = format!("{name}/system.txt");
            result.required_job_members.push(system.clone());
            if !inventory
                .members
                .get(&system)
                .is_some_and(|m| !m.directory && m.size > 0)
            {
                result.missing_job_members.push(system);
            }
            let suffix = format!("_{name}.txt");
            let aggregate = inventory
                .members
                .iter()
                .filter(|(member, _)| {
                    !member.contains('/')
                        && member.strip_suffix(&suffix).is_some_and(|index| {
                            !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit())
                        })
                })
                .collect::<Vec<_>>();
            match aggregate.as_slice() {
                [(member, m)] if !m.directory && m.size > 0 => {
                    result.required_job_members.push((*member).clone())
                }
                [] => result
                    .missing_job_members
                    .push(format!("<native-index>_{name}.txt")),
                _ => result.ambiguous_jobs.push(name.into()),
            }
        }
        let mut numbers = BTreeSet::new();
        for step in steps {
            let number = step["number"].as_u64().context("missing step number")?;
            let step_name = step["name"].as_str().context("missing step name")?;
            ensure!(
                number > 0 && numbers.insert(number) && step["status"] == "completed",
                "ambiguous or incomplete step"
            );
            if job["conclusion"] == "skipped" || step["conclusion"] == "skipped" {
                result
                    .skipped_steps
                    .push(format!("{id}:{number}:{step_name}"));
                continue;
            }
            let matching = exceptions
                .iter()
                .enumerate()
                .filter(|(_, e)| e.job == id && e.number == number && e.name == step_name)
                .collect::<Vec<_>>();
            ensure!(matching.len() <= 1, "duplicate non-emitting exception");
            if let Some((index, e)) = matching.first() {
                ensure!(
                    !e.reason.is_empty() && valid_digest(&e.evidence_sha256),
                    "non-emitting exception lacks evidence"
                );
                used.insert(*index);
                result.non_emitting_steps.push((*e).clone());
                continue;
            }
            if step_name.is_empty()
                || step_name.contains(['\\', ':'])
                || step_name.chars().any(char::is_control)
            {
                result
                    .ambiguous_steps
                    .push(format!("{id}:{number}:{step_name}"));
                continue;
            }
            // GitHub's simple action-path spelling is observed, not an immutability promise.
            let member = format!("{name}/{number}_{}.txt", step_name.replace('/', "_"));
            ensure!(names.insert(member.clone()), "ambiguous step mapping");
            result.required_members.push(member.clone());
            if !inventory
                .members
                .get(&member)
                .is_some_and(|m| !m.directory && m.size > 0)
            {
                result.missing_members.push(member);
            }
        }
        ensure!(
            job["conclusion"] == "skipped" || !steps.is_empty(),
            "executed job lacks a step inventory"
        );
    }
    ensure!(
        used.len() == exceptions.len(),
        "unbound non-emitting exception"
    );
    ensure!(
        !result.required_members.is_empty(),
        "empty completeness requirement"
    );
    Ok(result)
}

pub fn producer_matches(
    identity: &Identity,
    run: &Value,
    jobs: &Value,
    pr: Option<&Value>,
) -> Result<()> {
    identity.validate()?;
    ensure!(
        run["repository"]["full_name"] == identity.repository
            && run["repository"]["id"].as_u64() == Some(identity.repository_id)
            && run["head_repository"]["full_name"] == identity.repository
            && run["head_repository"]["id"].as_u64() == Some(identity.repository_id),
        "producer repository differs"
    );
    ensure!(
        run["id"].as_u64() == Some(identity.run)
            && run["run_attempt"].as_u64() == Some(1)
            && run["head_sha"] == identity.source
            && run["event"] == identity.event
            && run["head_branch"] == identity.branch
            && run["workflow_id"].as_u64() == Some(identity.workflow_id)
            && run["status"] == "completed"
            && run["conclusion"] == "success",
        "producer rerun, failure or source identity differs"
    );
    let path = run["path"].as_str().context("missing workflow path")?;
    let (file, reference) = path
        .split_once('@')
        .map_or((path, None), |(p, r)| (p, Some(r)));
    ensure!(
        file == identity.workflow
            && reference.is_none_or(|r| r == identity.workflow_revision
                || r == identity.branch
                || r == format!("refs/heads/{}", identity.branch)
                || identity
                    .pull_request
                    .as_ref()
                    .is_some_and(|p| r == format!("refs/pull/{}/merge", p.number))),
        "workflow reference differs"
    );
    let rows = jobs["jobs"].as_array().context("missing jobs")?;
    ensure!(
        !rows.is_empty() && jobs["total_count"].as_u64() == Some(rows.len() as u64),
        "incomplete native jobs"
    );
    for job in rows {
        ensure!(
            job["run_id"].as_u64() == Some(identity.run)
                && job["run_attempt"].as_u64() == Some(1)
                && job["head_sha"] == identity.source
                && job["status"] == "completed"
                && (job["conclusion"] == "success" || job["conclusion"] == "skipped"),
            "job eligibility differs"
        );
        for step in job["steps"].as_array().context("missing steps")? {
            ensure!(
                step["status"] == "completed"
                    && (step["conclusion"] == "success" || step["conclusion"] == "skipped"),
                "step eligibility differs"
            );
        }
    }
    if let Some(target) = &identity.pull_request {
        let associations = run["pull_requests"]
            .as_array()
            .context("missing PR associations")?;
        ensure!(
            associations.len() == 1
                && associations[0]["number"].as_u64() == Some(target.number)
                && associations[0]["head"]["sha"] == identity.source
                && associations[0]["base"]["sha"] == target.base,
            "run PR/base association differs"
        );
        let current = pr.context("fresh PR target unavailable")?;
        ensure!(
            current["state"] == "open"
                && current["number"].as_u64() == Some(target.number)
                && current["head"]["sha"] == identity.source
                && current["head"]["ref"] == identity.branch
                && current["base"]["sha"] == target.base
                && current["merge_commit_sha"] == identity.workflow_revision
                && current["base"]["ref"] == target.base_ref
                && current["head"]["repo"]["id"].as_u64() == Some(identity.repository_id)
                && current["base"]["repo"]["id"].as_u64() == Some(identity.repository_id),
            "current PR/source/base target stale"
        );
    }
    Ok(())
}

/// Stable attempt-specific identities, excluding mutable scheduling diagnostics.
pub fn job_identity(jobs: &Value) -> Result<Value> {
    let mut rows=jobs["jobs"].as_array().context("missing jobs")?.iter().map(|job| {
        let mut steps=job["steps"].as_array().context("missing steps")?.iter().map(|step|serde_json::json!({"number":step["number"],"name":step["name"],"status":step["status"],"conclusion":step["conclusion"]})).collect::<Vec<_>>();
        steps.sort_by_key(|step|step["number"].as_u64());
        Ok(serde_json::json!({"id":job["id"],"name":job["name"],"run_id":job["run_id"],"run_attempt":job["run_attempt"],"head_sha":job["head_sha"],"status":job["status"],"conclusion":job["conclusion"],"steps":steps}))
    }).collect::<Result<Vec<_>>>()?;
    rows.sort_by_key(|job| job["id"].as_u64());
    Ok(serde_json::json!({"total_count":jobs["total_count"],"jobs":rows}))
}

pub fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn read(path: &Path, budget: &mut Budget) -> Result<Vec<u8>> {
    let mut file = log_archive::regular_file(path)?;
    let mut raw = Vec::new();
    let mut buffer = [0; 65536];
    loop {
        budget.check()?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        ensure!(
            raw.len() + n <= MAX_JSON as usize,
            "evidence JSON exceeds limit"
        );
        budget.charge_bytes(n as u64)?;
        raw.extend_from_slice(&buffer[..n]);
    }
    Ok(raw)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObserverReceipt {
    pub schema: String,
    pub identity: Identity,
    pub native_endpoint: String,
    pub acquisition: String,
    pub observer_revision: String,
    pub capture_time_claim: String,
    pub archive: Inventory,
    pub jobs_sha256: String,
    pub non_emitting: Vec<NonEmitting>,
}
/// Trust comes from an operator-selected keyring and fingerprint, never a producer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub keyring: PathBuf,
    pub fingerprint: String,
    pub observer_revision: String,
}
/// Only successful pinned-key verification can construct this authority token.
pub struct AuthenticatedReceipt {
    receipt: ObserverReceipt,
    digest: String,
    raw: Vec<u8>,
    signature: Vec<u8>,
    verified_non_emitting: BTreeSet<String>,
    keyring: Vec<u8>,
    signing_fingerprint: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthenticationReference {
    pub receipt_sha256: String,
    pub signature_sha256: String,
    pub keyring_sha256: String,
    pub signing_fingerprint: String,
    pub approved_observer_revision: String,
    pub capture_time_claim: String,
}
impl AuthenticatedReceipt {
    fn reference(&self) -> AuthenticationReference {
        AuthenticationReference {
            receipt_sha256: self.digest.clone(),
            signature_sha256: digest(&self.signature),
            keyring_sha256: digest(&self.keyring),
            signing_fingerprint: self.signing_fingerprint.clone(),
            approved_observer_revision: self.receipt.observer_revision.clone(),
            capture_time_claim: self.receipt.capture_time_claim.clone(),
        }
    }
    pub(super) fn verify_non_emitting(
        &mut self,
        paths: &[PathBuf],
        root: &Path,
        budget: &mut Budget,
    ) -> Result<()> {
        ensure!(
            paths.len() <= 100,
            "non-emitting evidence inventory exceeds bound"
        );
        for path in paths {
            let raw = read(path, budget)?;
            let hash = digest(&raw);
            ensure!(
                self.receipt
                    .non_emitting
                    .iter()
                    .any(|step| step.evidence_sha256 == hash),
                "non-emitting evidence not bound to signed receipt"
            );
            if self.verified_non_emitting.insert(hash.clone()) {
                super::log_reconcile::write_new(
                    &root.join(format!("{}-{hash}-non-emitting-evidence", self.digest)),
                    &raw,
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn retain(&self, root: &Path) -> Result<()> {
        super::log_reconcile::write_new(
            &root.join(format!("{}-observer.json", self.digest)),
            &self.raw,
        )?;
        super::log_reconcile::write_new(
            &root.join(format!("{}-observer.signature", self.digest)),
            &self.signature,
        )?;
        super::log_reconcile::write_new(
            &root.join(format!("{}-observer-keyring.gpg", self.digest)),
            &self.keyring,
        )?;
        Ok(())
    }
}
pub fn authenticate(
    receipt_path: &Path,
    signature: &Path,
    approval: &Approval,
    budget: &mut Budget,
) -> Result<AuthenticatedReceipt> {
    ensure!(
        approval.fingerprint.len() == 40
            && approval.fingerprint.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid approved observer fingerprint"
    );
    let raw = read(receipt_path, budget)?;
    let signature_raw = read(signature, budget)?;
    // Isolated exact-byte copies avoid verification/parsing races and operator agent state.
    let temp = tempfile::tempdir()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    }
    fs::write(temp.path().join("receipt"), &raw)?;
    fs::write(temp.path().join("signature"), &signature_raw)?;
    let keyring = read(&approval.keyring, budget)?;
    fs::write(temp.path().join("trusted.gpg"), &keyring)?;
    let status = crate::bounded_process::query(
        Command::new("gpgv")
            .arg("--homedir")
            .arg(temp.path())
            .arg("--status-fd=1")
            .arg("--keyring")
            .arg(temp.path().join("trusted.gpg"))
            .arg(temp.path().join("signature"))
            .arg(temp.path().join("receipt")),
        budget.remaining()?.min(std::time::Duration::from_secs(30)),
        64 * 1024,
    )?;
    let status = std::str::from_utf8(&status)?;
    let signatures = status
        .lines()
        .filter_map(|line| line.strip_prefix("[GNUPG:] VALIDSIG "))
        .collect::<Vec<_>>();
    ensure!(
        signatures.len() == 1
            && signatures[0]
                .split_whitespace()
                .next()
                .is_some_and(|key| key.eq_ignore_ascii_case(&approval.fingerprint)),
        "unapproved observer signature"
    );
    let fields = signatures[0].split_whitespace().collect::<Vec<_>>();
    ensure!(
        fields
            .get(7)
            .is_some_and(|hash| matches!(*hash, "8" | "9" | "10" | "11"))
            && fields.get(8) == Some(&"00"),
        "observer receipt requires an exact-byte SHA-2 signature"
    );
    let receipt: ObserverReceipt = serde_json::from_slice(&raw)?;
    ensure!(
        receipt.schema == "simit.native-log-observer.v1"
            && receipt.acquisition == "native-https-download"
            && receipt.observer_revision == approval.observer_revision
            && receipt.native_endpoint
                == format!(
                    "https://api.github.com/repos/{}/actions/runs/{}/attempts/1/logs",
                    receipt.identity.repository, receipt.identity.run
                ),
        "observer acquisition/binding differs"
    );
    receipt.identity.validate()?;
    Ok(AuthenticatedReceipt {
        receipt,
        digest: digest(&raw),
        raw,
        signature: signature_raw,
        verified_non_emitting: BTreeSet::new(),
        keyring,
        signing_fingerprint: approval.fingerprint.clone(),
    })
}

/// Atomic no-clobber exact-byte custody, followed by independent full read-back.
/// The caller owns a private, trusted storage root; a digest filename is not provenance.
pub fn persist(path: &Path, root: &Path, inventory: &Inventory) -> Result<PathBuf> {
    persist_with(path, root, inventory, &mut Budget::default())
}
pub fn persist_with(
    path: &Path,
    root: &Path,
    inventory: &Inventory,
    budget: &mut Budget,
) -> Result<PathBuf> {
    ensure!(
        valid_digest(&inventory.sha256) && inventory.bytes <= log_archive::Limits::default().bytes,
        "invalid custody inventory digest/byte bound"
    );
    ensure!(
        fs::symlink_metadata(root)?.file_type().is_dir(),
        "storage root is not a real directory"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(root)?;
        ensure!(
            metadata.uid() == rustix::process::geteuid().as_raw() && metadata.mode() & 0o077 == 0,
            "storage root must be private and owned by this observer"
        );
    }
    let destination = root.join(format!("{}.zip", inventory.sha256));
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    let mut source = log_archive::regular_file(path)?;
    let mut hash = Sha256::new();
    let mut total = 0;
    let mut buffer = [0; 65536];
    loop {
        budget.check()?;
        let n = source.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        budget.charge_bytes(n as u64)?;
        ensure!(total <= inventory.bytes, "original grew during persistence");
        hash.update(&buffer[..n]);
        temporary.write_all(&buffer[..n])?;
    }
    ensure!(
        total == inventory.bytes && hex::encode(hash.finalize()) == inventory.sha256,
        "original changed before persistence"
    );
    temporary.as_file().sync_all()?;
    budget.check()?;
    match temporary.persist_noclobber(&destination) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.error.into()),
    }
    #[cfg(unix)]
    File::open(root)?.sync_all()?;
    budget.check()?;
    ensure!(
        log_archive::inspect_with(&destination, budget)? == *inventory,
        "stored native bytes failed independent read-back"
    );
    Ok(destination)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultRecord {
    pub schema: String,
    pub identity: Identity,
    pub contract: Contract,
    pub custody_integrity: Verdict,
    pub custody_provenance: Verdict,
    pub custody_completeness: Verdict,
    pub producer_eligibility: Verdict,
    pub provider_archive_state: ArchiveState,
    pub comparison: Option<Comparison>,
    pub coverage: Option<Coverage>,
    pub selected_original_digest: Option<String>,
    pub supporting_receipt_digest: Option<String>,
    pub observer_authentication: Option<AuthenticationReference>,
    pub accepted: bool,
    pub blockers: Vec<String>,
}
// Raw inventories/verdicts are conditional facts, not a public acceptance API.
// Only the complete reconciliation engine may turn this policy result into success.
pub(super) fn evaluate(
    identity: &Identity,
    original: Option<&Inventory>,
    observed: Option<&Inventory>,
    authenticated: Option<&AuthenticatedReceipt>,
    jobs_bytes: &[u8],
    eligible: Verdict,
    contract: Contract,
) -> Result<ResultRecord> {
    identity.validate()?;
    let mut result = ResultRecord {
        schema: SCHEMA.into(),
        identity: identity.clone(),
        contract,
        custody_integrity: Verdict::Unverified,
        custody_provenance: Verdict::Unverified,
        custody_completeness: Verdict::Unverified,
        producer_eligibility: eligible,
        provider_archive_state: ArchiveState::Unavailable,
        comparison: None,
        coverage: None,
        selected_original_digest: original.map(|i| i.sha256.clone()),
        supporting_receipt_digest: None,
        observer_authentication: authenticated.map(AuthenticatedReceipt::reference),
        accepted: false,
        blockers: vec![],
    };
    if let Some(native) = original {
        result.custody_integrity = Verdict::Verified;
        let exceptions = authenticated.map_or(&[][..], |r| r.receipt.non_emitting.as_slice());
        if let Some(proof) = authenticated {
            let receipt = &proof.receipt;
            result.supporting_receipt_digest = Some(proof.digest.clone());
            result.custody_provenance = if receipt.identity == *identity
                && receipt.archive == *native
                && receipt.jobs_sha256 == digest(jobs_bytes)
            {
                Verdict::Verified
            } else {
                Verdict::Failed
            };
            if receipt.archive != *native {
                result.custody_integrity = Verdict::Failed;
            }
        }
        let jobs: Value = serde_json::from_slice(jobs_bytes)?;
        let bound = jobs["jobs"].as_array().is_some_and(|rows| {
            rows.iter().all(|job| {
                job["run_id"].as_u64() == Some(identity.run)
                    && job["run_attempt"].as_u64() == Some(1)
                    && job["head_sha"] == identity.source
            })
        });
        match coverage(&jobs, native, exceptions) {
            Ok(coverage) => {
                result.custody_completeness = if coverage.missing_members.is_empty()
                    && coverage.ambiguous_steps.is_empty()
                    && coverage.missing_job_members.is_empty()
                    && coverage.ambiguous_jobs.is_empty()
                {
                    Verdict::Verified
                } else {
                    Verdict::Unverified
                };
                result.coverage = Some(coverage);
                if authenticated.is_some_and(|proof| {
                    proof
                        .receipt
                        .non_emitting
                        .iter()
                        .any(|step| !proof.verified_non_emitting.contains(&step.evidence_sha256))
                }) {
                    result.custody_completeness = Verdict::Unverified;
                    result.blockers.push(
                        "signed non-emitting exception lacks independent evidence read-back".into(),
                    );
                }
            }
            Err(_) => result
                .blockers
                .push("attempt-specific coverage mapping invalid or unsupported".into()),
        }
        if !bound {
            result.custody_completeness = Verdict::Failed;
            result
                .blockers
                .push("retained job inventory belongs to another run/attempt/source".into());
        }
        if let Some(fresh) = observed {
            let comparison = log_archive::compare(native, fresh);
            result.provider_archive_state = comparison.state.clone();
            result.comparison = Some(comparison);
        }
    }
    let permitted = matches!(
        result.provider_archive_state,
        ArchiveState::IdenticalRawArchive | ArchiveState::EquivalentMemberContent
    ) || (contract == Contract::RetainedNativeV1
        && result.provider_archive_state == ArchiveState::OmissionOnlyRegression);
    result.accepted = permitted
        && [
            &result.custody_integrity,
            &result.custody_provenance,
            &result.custody_completeness,
            &result.producer_eligibility,
        ]
        .into_iter()
        .all(|v| *v == Verdict::Verified);
    if result.custody_provenance != Verdict::Verified {
        result.blockers.push(
            "approved authenticated original acquisition receipt absent or incorrectly bound"
                .into(),
        );
    }
    if result.custody_completeness != Verdict::Verified {
        result
            .blockers
            .push("original completeness not demonstrated".into());
    }
    if result.producer_eligibility != Verdict::Verified {
        result
            .blockers
            .push("fresh producer/workflow/source/target eligibility not established".into());
    }
    if !permitted {
        result
            .blockers
            .push("provider observation does not satisfy selected explicit contract".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use log_archive::Member;
    use serde_json::json;
    use std::collections::BTreeMap;
    fn identity() -> Identity {
        Identity {
            repository: "example/project".into(),
            repository_id: 1,
            run: 42,
            attempt: 1,
            source: "a".repeat(40),
            event: "push".into(),
            branch: "trunk".into(),
            workflow: ".github/workflows/ci.yaml".into(),
            workflow_id: 7,
            workflow_revision: "a".repeat(40),
            workflow_blob: "b".repeat(40),
            pull_request: None,
        }
    }
    fn inventory(count: usize) -> Inventory {
        Inventory {
            sha256: format!("{count:064x}"),
            bytes: 100,
            members: (1..=count)
                .map(|i| {
                    (
                        format!("job/{i}_step.txt"),
                        Member {
                            raw_name_hex: hex::encode(format!("job/{i}_step.txt")),
                            size: 10,
                            sha256: "c".repeat(64),
                            directory: false,
                        },
                    )
                })
                .chain(["job/system.txt", "0_job.txt"].into_iter().map(|name| {
                    (
                        name.into(),
                        Member {
                            raw_name_hex: hex::encode(name),
                            size: 10,
                            sha256: "c".repeat(64),
                            directory: false,
                        },
                    )
                }))
                .collect::<BTreeMap<_, _>>(),
        }
    }
    fn jobs(count: usize) -> Vec<u8> {
        serde_json::to_vec(&json!({"total_count":1,"jobs":[{"id":5,"name":"job","run_id":42,"run_attempt":1,"head_sha":"a".repeat(40),"status":"completed","conclusion":"success","steps":(1..=count).map(|i|json!({"number":i,"name":"step","status":"completed","conclusion":"success"})).collect::<Vec<_>>()}]})).unwrap()
    }
    fn proof(identity: &Identity, native: &Inventory, jobs: &[u8]) -> AuthenticatedReceipt {
        AuthenticatedReceipt {
            receipt: ObserverReceipt {
                schema: "simit.native-log-observer.v1".into(),
                identity: identity.clone(),
                native_endpoint: "fixture".into(),
                acquisition: "fixture".into(),
                observer_revision: "fixture".into(),
                capture_time_claim: "untrusted-clock-claim".into(),
                archive: native.clone(),
                jobs_sha256: digest(jobs),
                non_emitting: vec![],
            },
            digest: "d".repeat(64),
            raw: vec![],
            signature: vec![],
            verified_non_emitting: BTreeSet::new(),
            keyring: vec![],
            signing_fingerprint: "fixture".into(),
        }
    }
    #[test]
    fn explicit_policy_is_required_and_partial_first_capture_cannot_pass() {
        let id = identity();
        let native = inventory(19);
        let fresh = inventory(2);
        let jobs = jobs(19);
        let authenticated = proof(&id, &native, &jobs);
        let strict = evaluate(
            &id,
            Some(&native),
            Some(&fresh),
            Some(&authenticated),
            &jobs,
            Verdict::Verified,
            Contract::StrictFreshV1,
        )
        .unwrap();
        assert!(!strict.accepted);
        assert_eq!(strict.custody_completeness, Verdict::Verified);
        assert!(
            evaluate(
                &id,
                Some(&native),
                Some(&fresh),
                Some(&authenticated),
                &jobs,
                Verdict::Verified,
                Contract::RetainedNativeV1
            )
            .unwrap()
            .accepted
        );
        let partial = proof(&id, &fresh, &jobs);
        assert!(
            !evaluate(
                &id,
                Some(&fresh),
                Some(&fresh),
                Some(&partial),
                &jobs,
                Verdict::Verified,
                Contract::RetainedNativeV1
            )
            .unwrap()
            .accepted
        );
        assert!(
            !evaluate(
                &id,
                Some(&native),
                Some(&fresh),
                None,
                &jobs,
                Verdict::Verified,
                Contract::RetainedNativeV1
            )
            .unwrap()
            .accepted
        );
    }
    #[test]
    fn every_original_binding_and_current_revocation_remains_required() {
        let id = identity();
        let native = inventory(2);
        let jobs = jobs(2);
        for field in [
            "repository",
            "repository_id",
            "run",
            "attempt",
            "source",
            "event",
            "branch",
            "workflow",
            "workflow_id",
            "workflow_revision",
            "workflow_blob",
            "pull_request",
        ] {
            let mut proof = proof(&id, &native, &jobs);
            let mut value = serde_json::to_value(&id).unwrap();
            value[field] = match field {
                "repository_id" | "run" | "attempt" | "workflow_id" => json!(999),
                "pull_request" => json!({"number":3,"base":"e".repeat(40),"base_ref":"trunk"}),
                _ => json!("different"),
            };
            proof.receipt.identity = serde_json::from_value(value).unwrap();
            assert!(
                !evaluate(
                    &id,
                    Some(&native),
                    Some(&native),
                    Some(&proof),
                    &jobs,
                    Verdict::Verified,
                    Contract::RetainedNativeV1
                )
                .unwrap()
                .accepted,
                "{field}"
            );
        }
        let proof = proof(&id, &native, &jobs);
        for eligible in [Verdict::Failed, Verdict::Unverified] {
            assert!(
                !evaluate(
                    &id,
                    Some(&native),
                    Some(&native),
                    Some(&proof),
                    &jobs,
                    eligible,
                    Contract::RetainedNativeV1
                )
                .unwrap()
                .accepted
            );
        }
        let mut proof = proof;
        proof.receipt.jobs_sha256 = "f".repeat(64);
        assert!(
            !evaluate(
                &id,
                Some(&native),
                Some(&native),
                Some(&proof),
                &jobs,
                Verdict::Verified,
                Contract::RetainedNativeV1
            )
            .unwrap()
            .accepted
        );
    }
    #[test]
    fn skipped_non_emitting_and_ambiguous_steps_are_explicit() {
        let native = inventory(2);
        let mut jobs: Value = serde_json::from_slice(&jobs(3)).unwrap();
        jobs["jobs"][0]["steps"][2]["conclusion"] = json!("skipped");
        let covered = coverage(&jobs, &native, &[]).unwrap();
        assert!(covered.missing_members.is_empty());
        assert_eq!(covered.skipped_steps.len(), 1);
        jobs["jobs"][0]["steps"][2]["conclusion"] = json!("success");
        let exception = NonEmitting {
            job: 5,
            number: 3,
            name: "step".into(),
            reason: "independently documented non-emitting step".into(),
            evidence_sha256: "f".repeat(64),
        };
        assert!(
            coverage(&jobs, &native, &[exception])
                .unwrap()
                .missing_members
                .is_empty()
        );
        jobs["jobs"][0]["steps"][2]["name"] = json!("unsupported: name");
        assert_eq!(
            coverage(&jobs, &native, &[]).unwrap().ambiguous_steps.len(),
            1
        );
        jobs["jobs"][0]["steps"][2]["number"] = json!(1);
        assert!(coverage(&jobs, &native, &[]).is_err());
    }
    #[test]
    fn executed_jobs_require_unambiguous_native_system_and_aggregate_members() {
        let id = identity();
        let mut native = inventory(2);
        let jobs = jobs(2);
        for name in ["job/system.txt", "0_job.txt"] {
            let removed = native.members.remove(name).unwrap();
            let proof = proof(&id, &native, &jobs);
            let result = evaluate(
                &id,
                Some(&native),
                Some(&native),
                Some(&proof),
                &jobs,
                Verdict::Verified,
                Contract::RetainedNativeV1,
            )
            .unwrap();
            assert!(!result.accepted);
            assert_eq!(result.coverage.unwrap().missing_job_members.len(), 1);
            native.members.insert(name.into(), removed);
        }
        native
            .members
            .insert("1_job.txt".into(), native.members["0_job.txt"].clone());
        assert_eq!(
            coverage(&serde_json::from_slice(&jobs).unwrap(), &native, &[])
                .unwrap()
                .ambiguous_jobs,
            vec!["job"]
        );
    }
    #[test]
    fn non_emitting_placeholder_requires_independently_retained_signed_evidence() {
        let id = identity();
        let native = inventory(2);
        let jobs = jobs(3);
        let mut proof = proof(&id, &native, &jobs);
        let evidence = b"fixture: independently observed step 3 emitted no native log";
        proof.receipt.non_emitting.push(NonEmitting {
            job: 5,
            number: 3,
            name: "step".into(),
            reason: "explicit observer evidence".into(),
            evidence_sha256: digest(evidence),
        });
        assert!(
            !evaluate(
                &id,
                Some(&native),
                Some(&native),
                Some(&proof),
                &jobs,
                Verdict::Verified,
                Contract::RetainedNativeV1
            )
            .unwrap()
            .accepted
        );
        let source = tempfile::tempdir().unwrap();
        let retained = tempfile::tempdir().unwrap();
        let path = source.path().join("non-emitting-evidence");
        fs::write(&path, evidence).unwrap();
        proof
            .verify_non_emitting(&[path], retained.path(), &mut Budget::default())
            .unwrap();
        assert!(
            evaluate(
                &id,
                Some(&native),
                Some(&native),
                Some(&proof),
                &jobs,
                Verdict::Verified,
                Contract::RetainedNativeV1
            )
            .unwrap()
            .accepted
        );
    }
    #[test]
    fn producer_reruns_failures_wrong_workflow_and_stale_pr_revoke() {
        let id = identity();
        let jobs: Value = serde_json::from_slice(&jobs(2)).unwrap();
        let run = json!({"id":42,"repository":{"id":1,"full_name":"example/project"},"head_repository":{"id":1,"full_name":"example/project"},"run_attempt":1,"head_sha":"a".repeat(40),"event":"push","head_branch":"trunk","workflow_id":7,"path":".github/workflows/ci.yaml","status":"completed","conclusion":"success"});
        producer_matches(&id, &run, &jobs, None).unwrap();
        for (field, value) in [
            ("id", json!(43)),
            ("repository", json!({"id":99,"full_name":"other/project"})),
            (
                "head_repository",
                json!({"id":99,"full_name":"other/project"}),
            ),
            ("run_attempt", json!(2)),
            ("conclusion", json!("failure")),
            ("head_sha", json!("f".repeat(40))),
            ("event", json!("pull_request")),
            ("path", json!(".github/workflows/wrong.yaml")),
        ] {
            let mut changed = run.clone();
            changed[field] = value;
            assert!(producer_matches(&id, &changed, &jobs, None).is_err());
        }
        let mut pr_id = id;
        pr_id.event = "pull_request".into();
        pr_id.pull_request = Some(Target {
            number: 3,
            base: "e".repeat(40),
            base_ref: "trunk".into(),
        });
        assert!(producer_matches(&pr_id, &run, &jobs, Some(&json!({"state":"closed"}))).is_err());
        let mut pr_run = run;
        pr_run["event"] = json!("pull_request");
        pr_run["pull_requests"] =
            json!([{"number":3,"head":{"sha":"a".repeat(40)},"base":{"sha":"e".repeat(40)}}]);
        let pr = json!({"state":"open","number":3,"merge_commit_sha":"a".repeat(40),"head":{"ref":"trunk","sha":"a".repeat(40),"repo":{"id":1}},"base":{"sha":"e".repeat(40),"ref":"trunk","repo":{"id":1}}});
        producer_matches(&pr_id, &pr_run, &jobs, Some(&pr)).unwrap();
        for field in ["head", "base"] {
            let mut changed = pr.clone();
            changed[field]["sha"] = json!("f".repeat(40));
            assert!(producer_matches(&pr_id, &pr_run, &jobs, Some(&changed)).is_err());
        }
        let mut changed_jobs = jobs.clone();
        changed_jobs["jobs"][0]["steps"][0]["name"] = json!("changed step");
        assert_ne!(
            job_identity(&jobs).unwrap(),
            job_identity(&changed_jobs).unwrap()
        );
    }
}
