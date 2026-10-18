//! Native replacement for external qualification-consumer archive equality scripts.
use super::{
    log_archive::{self, ArchiveState, Budget, Inventory},
    log_custody::{self, Approval, Contract, Identity, Target, Verdict},
};
use anyhow::{Context, Result, ensure};
use clap::Args as ClapArgs;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};
#[derive(Debug)]
struct ProviderUnavailable;
impl std::fmt::Display for ProviderUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("fresh provider evidence unavailable")
    }
}
impl std::error::Error for ProviderUnavailable {}

// Injection is private to this engine and its offline tests. Library callers use
// run(), which always selects the credential-isolated native HTTPS transport.
trait EvidenceSource {
    fn metadata(
        &mut self,
        identity: &Identity,
        endpoint: &str,
        budget: &mut Budget,
    ) -> Result<Vec<u8>>;
    fn capture(
        &mut self,
        identity: &Identity,
        root: &Path,
        budget: &mut Budget,
    ) -> super::log_http::CaptureOutcome;
}
struct NativeSource;
impl EvidenceSource for NativeSource {
    fn metadata(
        &mut self,
        identity: &Identity,
        endpoint: &str,
        budget: &mut Budget,
    ) -> Result<Vec<u8>> {
        super::log_http::metadata(identity, endpoint, budget).context(ProviderUnavailable)
    }
    fn capture(
        &mut self,
        identity: &Identity,
        root: &Path,
        budget: &mut Budget,
    ) -> super::log_http::CaptureOutcome {
        super::log_http::capture(identity, root, budget)
    }
}

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Versioned source-bound manifest; archives/receipts are inert local paths.
    #[arg(
        long,
        conflicts_with = "legacy_root",
        required_unless_present = "legacy_root"
    )]
    pub manifest: Option<PathBuf>,
    /// Read-only import of existing hosted-originals/*/qualification-runs.json captures.
    #[arg(long, requires_all=["source","base","checkout"])]
    pub legacy_root: Option<PathBuf>,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long)]
    pub base: Option<String>,
    /// Git object database used only to resolve immutable workflow blobs.
    #[arg(long)]
    pub checkout: Option<PathBuf>,
    /// Claimed executed PR workflow revision, separately checked against source/base parents.
    #[arg(long)]
    pub pr_revision: Option<String>,
    /// New private output directory. Existing results are never overwritten.
    #[arg(long)]
    pub results: PathBuf,
    #[arg(long, value_enum, default_value = "strict-fresh-v1")]
    pub contract: Contract,
    /// Operator-owned approval, outside candidate control; signature verification still required.
    #[arg(long)]
    pub observer_approval: Option<PathBuf>,
    /// Refresh exact run/attempt/jobs/workflow/target metadata via bounded gh API queries.
    #[arg(long)]
    pub refresh_producers: bool,
    /// Independently acquire current native attempt-1 ZIPs, preserving every older capture.
    #[arg(long, requires = "refresh_producers")]
    pub refresh_archives: bool,
    /// Additional inert acquisition/observer records, hashed without granting authority.
    #[arg(long)]
    pub supporting_record: Vec<PathBuf>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: String,
    pub originals: Vec<Original>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Original {
    pub identity: Identity,
    pub archive: PathBuf,
    pub jobs: PathBuf,
    pub observations: Vec<PathBuf>,
    pub supporting_records: Vec<PathBuf>,
    pub receipt: Option<PathBuf>,
    pub signature: Option<PathBuf>,
    /// Historical checksum record: integrity comparison only, never authentication.
    #[serde(default)]
    pub capture_record: Option<PathBuf>,
    #[serde(default)]
    pub non_emitting_evidence: Vec<PathBuf>,
}
fn json_file(path: &Path, budget: &mut Budget) -> Result<Value> {
    Ok(serde_json::from_slice(&log_custody::read(path, budget)?)?)
}
fn text(value: &Value, key: &str) -> Result<String> {
    Ok(value
        .pointer(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing {key}"))?
        .into())
}
fn basename(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}
fn number(value: &Value, key: &str) -> Result<u64> {
    value
        .pointer(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("missing {key}"))
}
#[derive(Default)]
struct ReportBudget(usize);
impl Write for ReportBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > (16 * 1024 * 1024_usize).saturating_sub(self.0) {
            return Err(std::io::Error::other("serialized report budget exhausted"));
        }
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl ReportBudget {
    fn charge(&mut self, value: &impl Serialize) -> Result<()> {
        Ok(serde_json::to_writer(self, value)?)
    }
}
fn git(checkout: &Path, arguments: &[&str], budget: &Budget) -> Result<String> {
    Ok(String::from_utf8(crate::bounded_process::query(
        Command::new("git").arg("-C").arg(checkout).args(arguments),
        budget.remaining()?,
        1024 * 1024,
    )?)?
    .trim()
    .into())
}
fn visit(root: &Path, depth: usize, entries: &mut usize, files: &mut Vec<PathBuf>) -> Result<()> {
    ensure!(depth < 12, "capture directory depth exceeds bound");
    for entry in fs::read_dir(root)? {
        *entries += 1;
        ensure!(*entries <= 4096, "capture file inventory exceeds bound");
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            visit(&entry.path(), depth + 1, entries, files)?;
        } else if kind.is_file() {
            files.push(entry.path());
        }
        // Symlinks are not acquisition or storage evidence.
    }
    Ok(())
}

fn import_legacy(args: &Args, budget: &mut Budget) -> Result<Manifest> {
    let root = args.legacy_root.as_ref().context("missing legacy root")?;
    let source = args.source.as_ref().context("missing source")?;
    let base = args.base.as_ref().context("missing base")?;
    let checkout = args.checkout.as_ref().context("missing checkout")?;
    ensure!(
        source.len() == 40
            && base.len() == 40
            && source
                .bytes()
                .chain(base.bytes())
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid source/base revisions"
    );
    if let Some(revision) = &args.pr_revision {
        ensure!(
            revision.len() == 40
                && revision
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid PR revision"
        );
        ensure!(
            git(
                checkout,
                &["show", "--format=%P", "--no-patch", revision],
                budget
            )? == format!("{base} {source}"),
            "claimed PR revision parents differ"
        );
        ensure!(
            git(
                checkout,
                &["rev-parse", &format!("{revision}^{{tree}}")],
                budget
            )? == git(
                checkout,
                &["rev-parse", &format!("{source}^{{tree}}")],
                budget
            )?,
            "claimed PR tree differs"
        );
    }
    let mut files = vec![];
    visit(root, 0, &mut 0, &mut files)?;
    files.sort();
    let mut originals = vec![];
    let mut matrix = BTreeSet::new();
    for selection in &files {
        if selection.file_name().and_then(|s| s.to_str()) != Some("qualification-runs.json")
            || !selection.starts_with(root.join("hosted-originals"))
        {
            continue;
        }
        let folder = selection.parent().context("missing run folder")?;
        let rows = json_file(selection, budget)?;
        for row in rows.as_array().context("invalid qualification inventory")? {
            let run = number(row, "/id")?;
            let terminal_path = folder.join(format!("{run}-terminal.json"));
            let terminal = json_file(&terminal_path, budget)?;
            ensure!(
                row["head_sha"] == *source
                    && terminal["head_sha"] == *source
                    && row["run_attempt"] == 1
                    && terminal["run_attempt"] == 1
                    && row["event"] == terminal["event"]
                    && row["path"] == terminal["path"],
                "legacy selection/source/attempt differs"
            );
            let event = text(&terminal, "/event")?;
            let workflow = text(&terminal, "/path")?
                .split('@')
                .next()
                .context("missing path")?
                .to_owned();
            ensure!(
                matrix.insert((workflow.clone(), event.clone())),
                "duplicate qualification selection"
            );
            let target = if event == "pull_request" {
                let prs = terminal["pull_requests"]
                    .as_array()
                    .context("missing PR associations")?;
                ensure!(prs.len() == 1, "ambiguous PR association");
                ensure!(
                    prs[0]["base"]["sha"] == *base && prs[0]["head"]["sha"] == *source,
                    "historical PR/base differs"
                );
                Some(Target {
                    number: number(&prs[0], "/number")?,
                    base: base.clone(),
                    base_ref: text(&prs[0], "/base/ref")?,
                })
            } else {
                None
            };
            let revision = if event == "pull_request" {
                args.pr_revision.as_ref().unwrap_or(source)
            } else {
                source
            };
            let blob = git(
                checkout,
                &["rev-parse", &format!("{revision}:{workflow}")],
                budget,
            )?;
            let identity = Identity {
                repository: text(&terminal, "/repository/full_name")?,
                repository_id: number(&terminal, "/repository/id")?,
                run,
                attempt: 1,
                source: source.clone(),
                event,
                branch: text(&terminal, "/head_branch")?,
                workflow,
                workflow_id: number(&terminal, "/workflow_id")?,
                workflow_revision: revision.clone(),
                workflow_blob: blob,
                pull_request: target,
            };
            identity.validate()?;
            let archive = folder.join(format!("{run}-logs.zip"));
            let observations = files
                .iter()
                .filter(|path| {
                    path.extension().and_then(|s| s.to_str()) == Some("zip")
                        && path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .is_some_and(|name| name.starts_with(&format!("{run}-")))
                        && **path != archive
                })
                .cloned()
                .collect();
            let mut supporting_records = vec![
                selection.clone(),
                terminal_path,
                folder.join(format!("{run}-custody.json")),
                folder.join(format!("{run}-watch.log")),
            ];
            for name in [
                "observer-scope.json",
                "hosted-observation.json",
                "watch-originals.py",
                "signed-successor-handoff.json",
                "source-resolution.json",
                "merge-blocker.json",
            ] {
                let path = root.join(name);
                if path.is_file() {
                    supporting_records.push(path);
                }
            }
            supporting_records.extend(args.supporting_record.iter().cloned());
            supporting_records.extend(
                files
                    .iter()
                    .filter(|path| {
                        path.extension().and_then(|s| s.to_str()) == Some("json")
                            && path.file_name().and_then(|s| s.to_str())
                                == Some(&format!("{run}-receipt.json"))
                    })
                    .cloned(),
            );
            originals.push(Original {
                identity,
                archive,
                jobs: folder.join(format!("{run}-jobs.json")),
                observations,
                supporting_records,
                receipt: None,
                signature: None,
                capture_record: Some(folder.join(format!("{run}-custody.json"))),
                non_emitting_evidence: vec![],
            });
        }
    }
    ensure!(
        !originals.is_empty() && originals.len() <= 100,
        "missing qualification inventory"
    );
    Ok(Manifest {
        schema: "simit.native-log-manifest.v1".into(),
        originals,
    })
}

fn provider(
    identity: &Identity,
    endpoint: &str,
    budget: &mut Budget,
    root: &Path,
    label: &str,
    source: &mut impl EvidenceSource,
) -> Result<Value> {
    let raw = source.metadata(identity, endpoint, budget)?;
    let value = serde_json::from_slice(&raw)?;
    let path = root.join(format!("{}-{label}.json", identity.run));
    write_new(&path, &raw)?;
    write_json_new(
        &root.join(format!("{}-{label}-reference.json", identity.run)),
        &json!({"name":basename(&path),"sha256":log_custody::digest(&raw),"bytes":raw.len(),
            "endpoint":format!("https://api.github.com/repos/{}/{endpoint}",identity.repository),
            "observed_at_claim":chrono::Utc::now().to_rfc3339(),
            "acquisition":"current exact-endpoint native HTTPS query by this engine; private response read-back"}),
    )?;
    Ok(value)
}
fn eligible(
    identity: &Identity,
    retained_jobs: &Value,
    budget: &mut Budget,
    root: &Path,
    label: &str,
    source: &mut impl EvidenceSource,
) -> Result<()> {
    let run = provider(
        identity,
        &format!("actions/runs/{}", identity.run),
        budget,
        root,
        &format!("{label}-run"),
        source,
    )?;
    let jobs = provider(
        identity,
        &format!("actions/runs/{}/attempts/1/jobs?per_page=100", identity.run),
        budget,
        root,
        &format!("{label}-jobs"),
        source,
    )?;
    let pr = identity
        .pull_request
        .as_ref()
        .map(|target| {
            provider(
                identity,
                &format!("pulls/{}", target.number),
                budget,
                root,
                &format!("{label}-pr"),
                source,
            )
        })
        .transpose()?;
    log_custody::producer_matches(identity, &run, &jobs, pr.as_ref())?;
    ensure!(
        log_custody::job_identity(&jobs)? == log_custody::job_identity(retained_jobs)?,
        "current attempt-specific job/step inventory changed"
    );
    let workflow = provider(
        identity,
        &format!("actions/workflows/{}", identity.workflow_id),
        budget,
        root,
        &format!("{label}-workflow"),
        source,
    )?;
    ensure!(
        workflow["id"].as_u64() == Some(identity.workflow_id)
            && workflow["path"] == identity.workflow
            && workflow["state"] == "active",
        "current workflow enrollment differs"
    );
    let file = provider(
        identity,
        &format!(
            "contents/{}?ref={}",
            identity.workflow, identity.workflow_revision
        ),
        budget,
        root,
        &format!("{label}-workflow-blob"),
        source,
    )?;
    ensure!(
        file["sha"] == identity.workflow_blob,
        "workflow source blob differs"
    );
    if identity.event == "push" {
        let branch = provider(
            identity,
            &format!(
                "git/ref/heads/{}",
                url::form_urlencoded::byte_serialize(identity.branch.as_bytes())
                    .collect::<String>()
            ),
            budget,
            root,
            &format!("{label}-branch"),
            source,
        )?;
        ensure!(
            branch["object"]["sha"] == identity.source,
            "current source branch stale"
        );
    }
    // An authenticated observer receipt must bind executed revision; querying the
    // source file alone never turns it into proof of actual workflow execution.
    Ok(())
}
pub(super) fn write_new(path: &Path, raw: &[u8]) -> Result<()> {
    ensure!(
        raw.len() <= 16 * 1024 * 1024,
        "result evidence exceeds JSON bound"
    );
    let parent = path.parent().context("result lacks a parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(raw)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    ensure!(
        fs::read(path)? == raw,
        "result persistence read-back failed"
    );
    Ok(())
}

struct BoundedWriter<W> {
    inner: W,
    remaining: usize,
}
impl<W: Write> Write for BoundedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(std::io::Error::other("serialized evidence exceeds bound"));
        }
        let n = self.inner.write(bytes)?;
        self.remaining -= n;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
fn write_json_new(path: &Path, value: &impl Serialize) -> Result<()> {
    write_json_bounded(path, value, 16 * 1024 * 1024)
}
fn write_json_bounded(path: &Path, value: &impl Serialize, maximum: usize) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Seek};
    let parent = path.parent().context("result lacks a parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(
        BoundedWriter {
            inner: &mut temporary,
            remaining: maximum,
        },
        value,
    )?;
    temporary.as_file().sync_all()?;
    temporary.as_file_mut().rewind()?;
    let mut expected = Sha256::new();
    std::io::copy(temporary.as_file_mut(), &mut expected)?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    let mut stored = log_archive::regular_file(path)?;
    let mut actual = Sha256::new();
    let read = std::io::copy(
        &mut Read::by_ref(&mut stored).take(maximum as u64 + 1),
        &mut actual,
    )?;
    ensure!(
        read <= maximum as u64 && expected.finalize() == actual.finalize(),
        "result persistence read-back failed"
    );
    Ok(())
}

fn metadata_references(
    root: &Path,
    run: u64,
    phase: &str,
    budget: &mut Budget,
) -> Result<Vec<Value>> {
    ["run", "jobs", "pr", "workflow", "workflow-blob", "branch"]
        .into_iter()
        .filter_map(|label| {
            let path = root.join(format!("{run}-{phase}-{label}-reference.json"));
            path.is_file().then(|| json_file(&path, budget))
        })
        .collect()
}
fn reference(path: &Path, budget: &mut Budget) -> Result<Value> {
    let raw = log_custody::read(path, budget)?;
    let parsed = serde_json::from_slice::<Value>(&raw).ok();
    let http = parsed
        .as_ref()
        .filter(|v| v.get("apiStatus").is_some())
        .map(|value| {
            let mut safe = serde_json::Map::new();
            for key in [
                "apiStatus",
                "downloadStatus",
                "crossOriginAuthorizationForwarded",
            ] {
                if let Some(v) = value.get(key)
                    && ((key.ends_with("Status")
                        && v.as_u64()
                            .is_some_and(|status| (100..600).contains(&status)))
                        || (key == "crossOriginAuthorizationForwarded" && v.is_boolean()))
                {
                    safe.insert(key.into(), v.clone());
                }
            }
            for key in ["apiHeaders", "downloadHeaders"] {
                let mut headers = serde_json::Map::new();
                if let Some(values) = value[key].as_object() {
                    for (name, v) in values {
                        if v.as_str().is_some_and(|s| {
                            s.len() <= 256 && !s.contains(['?', '\r', '\n']) && !s.contains("://")
                        }) && [
                            "date",
                            "content-type",
                            "content-length",
                            "x-github-request-id",
                            "x-github-api-version-selected",
                        ]
                        .contains(&name.to_ascii_lowercase().as_str())
                        {
                            headers.insert(name.clone(), v.clone());
                        }
                    }
                }
                safe.insert(key.into(), Value::Object(headers));
            }
            Value::Object(safe)
        });
    Ok(
        json!({"name":basename(path),"sha256":log_custody::digest(&raw),"bytes":raw.len(),"http_metadata_claim":http,"authentication":"unsigned local supporting record; no authority","filesystem_time_claim":fs::metadata(path)?.modified().ok().and_then(|t|t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d|d.as_secs()),"time_provenance":"mutable local filesystem mtime; HTTP Date is an unsigned retained-header claim"}),
    )
}
fn observation(
    path: &Path,
    original: Option<&Inventory>,
    storage: &Path,
    budget: &mut Budget,
) -> Value {
    match log_archive::inspect_with(path, budget) {
        Ok(inventory) => match log_custody::persist_with(path, storage, &inventory, budget) {
            Ok(_) => {
                json!({"name":basename(path),"inventory":inventory,"comparison":original.map(|o|log_archive::compare(o,&inventory)),"archive_validation":"verified","storage_integrity":"verified","independent_read_back":true,"filesystem_time_claim":fs::metadata(path).ok().and_then(|m|m.modified().ok()).and_then(|t|t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d|d.as_secs()),"capture_time_provenance":"mutable filesystem mtime/path/record claims only"})
            }
            Err(error) => {
                json!({"name":basename(path),"inventory":inventory,"state":ArchiveState::Unavailable,"archive_validation":"verified","storage_integrity":"failed","independent_read_back":false,"failure":failure(&error,"storage_read_back"),"error":"native persistence or read-back failed"})
            }
        },
        Err(error) => {
            let raw = log_archive::raw_integrity(path, budget).ok();
            let unavailable =
                error.is::<log_archive::ResourceFailure>() || error.is::<std::io::Error>();
            json!({"name":basename(path),"raw_integrity":raw.map(|(bytes,sha256)|json!({"bytes":bytes,"sha256":sha256})),"state":if unavailable {ArchiveState::Unavailable}else{ArchiveState::Invalid},"archive_validation":if unavailable {"unverified"}else{"failed"},"failure":failure(&error,"archive_validation"),"error":"bounded ZIP validation failed or capture unavailable"})
        }
    }
}

pub(super) fn failure(error: &anyhow::Error, stage: &str) -> Value {
    let code = if let Some(resource) = error.downcast_ref::<log_archive::ResourceFailure>() {
        serde_json::to_value(resource).expect("fixed resource failure")
    } else if error.is::<std::io::Error>() {
        json!(if stage == "storage_read_back" {
            "storage_io"
        } else {
            "input_io"
        })
    } else {
        json!(match stage {
            "storage_read_back" => "storage_verification_failed",
            "archive_validation" => "archive_invalid_or_unsupported",
            "authentication" => "authentication_failed",
            _ => "evidence_operation_failed",
        })
    };
    json!({"stage":stage,"code":code})
}

/// Verified library entry point. Manifests, inventories and verdict enums alone
/// cannot authorize acceptance; the complete engine owns read-back and refresh.
pub fn run(args: Args) -> Result<()> {
    run_with_source(args, Budget::default(), &mut NativeSource)
}
fn run_with_source(args: Args, mut budget: Budget, source: &mut impl EvidenceSource) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&args.results)?;
    let mut stage = "input_validation";
    let result = reconcile(&args, &mut budget, source, &mut stage);
    if let Err(error) = &result {
        // A separate 16 KiB terminal allowance survives exhausted input budgets.
        // No existing result is replaced and no raw child/URL diagnostics escape.
        if !args.results.join("reconciliation.json").exists() {
            let terminal = json!({"schema":"simit.native-log-failure.v1","accepted":false,
                "failure":failure(error, stage),"budget_usage":budget.usage(),
                "candidate_qualification_only":true,"installed_observer_acceptance":false});
            if write_json_bounded(&args.results.join("failure.json"), &terminal, 16 * 1024).is_err()
            {
                return result.context("terminal failure evidence could not be persisted");
            }
        }
    }
    result
}
fn reconcile(
    args: &Args,
    budget: &mut Budget,
    source: &mut impl EvidenceSource,
    stage: &mut &str,
) -> Result<()> {
    let (engine_bytes, engine_digest) =
        log_archive::raw_integrity(&std::env::current_exe()?, budget)?;
    let manifest = match &args.manifest {
        Some(path) => serde_json::from_slice::<Manifest>(&log_custody::read(path, budget)?)?,
        None => import_legacy(args, budget)?,
    };
    ensure!(
        manifest.schema == "simit.native-log-manifest.v1"
            && !manifest.originals.is_empty()
            && manifest.originals.len() <= 100,
        "unsupported or empty manifest"
    );
    for original in &manifest.originals {
        ensure!(
            original.observations.len() <= 32 && original.supporting_records.len() <= 32,
            "producer capture/record inventory exceeds bound"
        );
        original.identity.validate()?;
    }
    let approval = args
        .observer_approval
        .as_ref()
        .map(|path| {
            log_custody::read(path, budget)
                .and_then(|raw| Ok(serde_json::from_slice::<Approval>(&raw)?))
        })
        .transpose()?;
    // Private new root, reused by every concurrent no-clobber native capture.
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    let storage = args.results.join("native-sha256");
    builder.create(&storage)?;
    let mut rows = vec![];
    let mut report_budget = ReportBudget::default();
    let mut runs = BTreeSet::new();
    for original in &manifest.originals {
        *stage = "original_validation";
        let id = &original.identity;
        id.validate()?;
        ensure!(
            runs.insert((id.repository_id, id.run)),
            "duplicate producer manifest entry"
        );
        let inspected = log_archive::inspect_with(&original.archive, budget);
        let original_validation_failure = inspected
            .as_ref()
            .err()
            .map(|error| failure(error, "archive_validation"));
        let native = inspected.ok();
        report_budget.charge(&native)?;
        *stage = "original_storage";
        let persisted = native
            .as_ref()
            .map(|native| log_custody::persist_with(&original.archive, &storage, native, budget));
        let storage_failure = persisted
            .as_ref()
            .and_then(|result| result.as_ref().err())
            .map(|error| failure(error, "storage_read_back"));
        let durable = persisted.is_some_and(|result| result.is_ok());
        *stage = "retained_jobs";
        let jobs_raw = log_custody::read(&original.jobs, budget)?;
        write_new(
            &args.results.join(format!("{}-retained-jobs.json", id.run)),
            &jobs_raw,
        )?;
        let retained_jobs: Value = serde_json::from_slice(&jobs_raw)?;
        let capture_matches = original.capture_record.as_ref().map(|path| {
            json_file(path, budget)
                .map(|capture| {
                    native.as_ref().is_some_and(|native| {
                        capture["run"].as_u64() == Some(id.run)
                            && capture["attempt"] == 1
                            && capture["head"] == id.source
                            && capture["event"] == id.event
                            && capture["logs_sha256"] == native.sha256
                            && capture["members"].as_object().is_some_and(|members| {
                                members.len() == native.members.len()
                                    && members.iter().all(|(name, sha)| {
                                        native.members.get(name).is_some_and(|m| sha == &m.sha256)
                                    })
                            })
                    })
                })
                .unwrap_or(false)
        });
        *stage = "authentication";
        let mut authentication_failure = None;
        let proof = match (&original.receipt, &original.signature, &approval) {
            (Some(receipt), Some(signature), Some(approval)) => {
                let result = log_custody::authenticate(receipt, signature, approval, budget)
                    .and_then(|mut proof| {
                        proof.verify_non_emitting(
                            &original.non_emitting_evidence,
                            &args.results,
                            budget,
                        )?;
                        proof.retain(&args.results)?;
                        Ok(proof)
                    });
                authentication_failure = result
                    .as_ref()
                    .err()
                    .map(|error| failure(error, "authentication"));
                result.ok()
            }
            _ => None,
        };
        let supplied_authentication_failed =
            (original.receipt.is_some() || original.signature.is_some()) && proof.is_none();
        *stage = "initial_eligibility";
        let (eligibility, eligibility_reason) = if args.refresh_producers {
            match eligible(id, &retained_jobs, budget, &args.results, "initial", source) {
                Ok(()) => (Verdict::Verified, None),
                Err(error) => (
                    if error.downcast_ref::<ProviderUnavailable>().is_some() {
                        Verdict::Unverified
                    } else {
                        Verdict::Failed
                    },
                    Some(error.to_string()),
                ),
            }
        } else {
            (
                Verdict::Unverified,
                Some("fresh provider queries not requested".into()),
            )
        };
        let fresh_jobs_path = args.results.join(format!("{}-initial-jobs.json", id.run));
        let fresh_job_inventory_matches = if args.refresh_producers && fresh_jobs_path.is_file() {
            json_file(&fresh_jobs_path, budget)
                .and_then(|fresh| {
                    Ok(log_custody::job_identity(&fresh)?
                        == log_custody::job_identity(&retained_jobs)?)
                })
                .ok()
        } else {
            None
        };
        let fresh_metadata_references =
            metadata_references(&args.results, id.run, "initial", budget)?;
        *stage = "archive_observations";
        let mut observations = vec![];
        for path in &original.observations {
            let value = observation(path, native.as_ref(), &storage, budget);
            report_budget.charge(&value)?;
            observations.push(value);
        }
        let current_observation = if args.refresh_archives {
            let capture = source.capture(id, &args.results, budget);
            let value = match (capture.path, capture.failure) {
                (Some(path), None) => {
                    let mut value = observation(&path, native.as_ref(), &storage, budget);
                    value["http_metadata"] = capture.metadata;
                    value
                }
                (path, error) => {
                    json!({"name":"current-native-attempt-1","partial_capture":path.as_deref().and_then(basename),"state":ArchiveState::Unavailable,"failure":error,"http_metadata":capture.metadata})
                }
            };
            report_budget.charge(&value)?;
            observations.push(value);
            Some(observations.len() - 1)
        } else {
            None
        };
        let records = original
            .supporting_records
            .iter()
            .map(|path| {
                reference(path, budget)
                    .unwrap_or_else(|_| json!({"name":basename(path),"state":"missing_or_invalid"}))
            })
            .collect::<Vec<_>>();
        let mut evaluations = vec![];
        // Every variant is evaluated; lexical order or largest membership cannot select a baseline.
        for (index, observation) in observations.iter().enumerate() {
            let observed = observation
                .get("inventory")
                .filter(|_| observation["independent_read_back"] == true)
                .map(|v| serde_json::from_value::<Inventory>(v.clone()))
                .transpose()?;
            let mut result = log_custody::evaluate(
                id,
                native.as_ref().filter(|_| durable),
                observed.as_ref(),
                proof.as_ref(),
                &jobs_raw,
                eligibility.clone(),
                args.contract,
            )?;
            // Coverage belongs to the selected original, not to each later observation.
            if index > 0 {
                result.coverage = None;
            }
            if observed.is_none() {
                result.provider_archive_state = if observation["state"] == "invalid" {
                    ArchiveState::Invalid
                } else {
                    ArchiveState::Unavailable
                };
            }
            if Some(index) != current_observation {
                result.accepted = false;
                result.blockers.push(
                    "historical observation only; no independent current native acquisition".into(),
                );
            }
            report_budget.charge(&result)?;
            evaluations.push(result);
        }
        if evaluations.is_empty() {
            evaluations.push(log_custody::evaluate(
                id,
                native.as_ref().filter(|_| durable),
                None,
                proof.as_ref(),
                &jobs_raw,
                eligibility,
                args.contract,
            )?);
        }
        if supplied_authentication_failed {
            for result in &mut evaluations {
                result.custody_provenance = Verdict::Failed;
                result.accepted = false;
                result
                    .blockers
                    .push("supplied original authentication or evidence retention failed".into());
            }
        }
        if capture_matches == Some(false) {
            for result in &mut evaluations {
                result.custody_integrity = Verdict::Failed;
                result.accepted = false;
                result
                    .blockers
                    .push("original differs from retained checksum/source capture record".into());
            }
        }
        let historical_conflict = evaluations.iter().any(|r| {
            matches!(
                r.provider_archive_state,
                ArchiveState::AddedOrChangedConflict | ArchiveState::Invalid
            )
        });
        let observation_failure = observations
            .iter()
            .any(|value| value.get("failure").is_some());
        if historical_conflict || observation_failure {
            for result in &mut evaluations {
                result.accepted = false;
                result.blockers.push(
                    if historical_conflict {
                        "retained archive conflict or invalid variant vetoes producer acceptance"
                    } else {
                        "an observation could not be independently validated and retained"
                    }
                    .into(),
                );
            }
        }
        let accepted =
            current_observation.is_some_and(|i| evaluations[i].accepted) && !historical_conflict;
        rows.push(json!({"identity":id,"accepted":accepted,"selected_original":native,"original_validation_failure":original_validation_failure,"storage_failure":storage_failure,"authentication_failure":authentication_failure,"durable_read_back":durable,"capture_record_integrity_match":capture_matches,"original_path_claim":original.archive,"jobs_digest":log_custody::digest(&jobs_raw),"fresh_job_inventory_matches":fresh_job_inventory_matches,"fresh_metadata_references":fresh_metadata_references,"final_metadata_references":[],"observations":observations,"supporting_records":records,"evaluations":evaluations,"eligibility_reason":eligibility_reason,"acquisition_provenance":"source-bound signed observer required; unsigned historical records do not establish native origin","workflow_revision_provenance":"operator-selected immutable revision; actual execution requires approved observer binding"}));
    }
    // Revalidate the complete selected set after all captures; per-row final
    // checks during collection would age while later siblings were inspected.
    for row in &mut rows {
        *stage = "final_eligibility";
        if row["accepted"] != true {
            continue;
        }
        let id: Identity = serde_json::from_value(row["identity"].clone())?;
        let jobs = json_file(
            &args.results.join(format!("{}-retained-jobs.json", id.run)),
            budget,
        )?;
        let final_eligibility = eligible(&id, &jobs, budget, &args.results, "final", source);
        row["final_metadata_references"] =
            json!(metadata_references(&args.results, id.run, "final", budget)?);
        if let Err(error) = final_eligibility {
            row["accepted"] = json!(false);
            row["eligibility_reason"] = json!(error.to_string());
            if let Some(results) = row["evaluations"].as_array_mut() {
                for result in results {
                    result["accepted"] = json!(false);
                    result["producer_eligibility"] = serde_json::to_value(
                        if error.downcast_ref::<ProviderUnavailable>().is_some() {
                            Verdict::Unverified
                        } else {
                            Verdict::Failed
                        },
                    )?;
                    result["blockers"]
                        .as_array_mut()
                        .context("missing result blockers")?
                        .push(json!(
                            "final producer/source/workflow/target revalidation failed"
                        ));
                }
            }
        }
    }
    let accepted = rows.iter().all(|row| row["accepted"] == true);
    *stage = "report_serialization";
    let report = json!({"schema":log_custody::SCHEMA,"contract":args.contract,"engine_executable_sha256":engine_digest,"engine_bytes":engine_bytes,"engine_provenance":"locally invoked reconciliation binary; checksum is not hosted qualification","manifest_digest":log_custody::digest(&serde_json::to_vec(&manifest)?),"originals":rows,"accepted":accepted,"candidate_qualification_only":true,"installed_observer_acceptance":false,"merge_acceptance":false,"producer_refresh_requested":args.refresh_producers,"current_native_acquisition_requested":args.refresh_archives,"provider_archive_observation":"each native capture retained separately, never reconstructed or unioned","generated_at_claim":chrono::Utc::now().to_rfc3339(),"timestamp_provenance":"local system clock; not trusted historical timestamp"});
    write_json_new(&args.results.join("manifest.json"), &manifest)?;
    // Redacted upstream bundle: metadata/digests only; no ZIP/log contents, credentials or signed URLs.
    let redacted=report["originals"].as_array().context("missing report rows")?.iter().map(|row| {
        json!({"identity":row["identity"],"accepted":row["accepted"],"original_digest":row["selected_original"]["sha256"],"original_inventory":row["selected_original"],"original_members":row["selected_original"]["members"].as_object().map(|m|m.len()),"fresh_job_inventory_matches":row["fresh_job_inventory_matches"],"fresh_metadata_references":row["fresh_metadata_references"],"final_metadata_references":row["final_metadata_references"],"observations":row["observations"],"supporting_records":row["supporting_records"],"evaluations":row["evaluations"],"eligibility_reason":row["eligibility_reason"]})
    }).collect::<Vec<_>>();
    write_json_new(
        &args.results.join("upstream-incident-bundle.json"),
        &json!({"schema":"simit.native-log-incident.v1","records":redacted,"accepted":accepted,"retention":"No account retention setting or expiry cause inferred. Current policy/expiry require GitHub investigation.","native_bytes":"exact bytes retained privately in digest-addressed custody","publication":false}),
    )?;
    budget.check()?;
    // The acceptance-bearing diagnostic is the final atomic write. Callers also
    // require command success; a report by itself grants no authority.
    write_json_new(&args.results.join("reconciliation.json"), &report)?;
    println!(
        "{}: {} originals reconciled; accepted={accepted}",
        args.results.display(),
        manifest.originals.len()
    );
    ensure!(
        accepted,
        "selected custody contract not satisfied; see reconciliation.json"
    );
    Ok(())
}

#[cfg(test)]
mod followup_tests {
    use super::*;

    fn fixture_identity() -> Identity {
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
    fn fixture_jobs() -> Value {
        json!({"total_count":1,"jobs":[{"id":5,"name":"job","run_id":42,"run_attempt":1,
            "head_sha":"a".repeat(40),"status":"completed","conclusion":"success",
            "steps":[{"number":1,"name":"step","status":"completed","conclusion":"success"}]}]})
    }
    fn fixture_archive(path: &Path, step: bool, changed: bool, compressed: bool) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for name in ["0_job.txt", "job/system.txt", "job/1_step.txt"] {
            if name == "job/1_step.txt" && !step {
                continue;
            }
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(if compressed {
                        zip::CompressionMethod::Deflated
                    } else {
                        zip::CompressionMethod::Stored
                    })
                    .last_modified_time(zip::DateTime::default()),
            )
            .unwrap();
            zip.write_all(if changed {
                b"changed fixture"
            } else {
                b"synthetic native fixture\n"
            })
            .unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn acceptance_receipt_binds_exact_synthetic_bytes() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("native.zip");
        fixture_archive(&archive, true, false, false);
        let receipt = log_custody::ObserverReceipt {
            schema: "simit.native-log-observer.v1".into(),
            identity: fixture_identity(),
            native_endpoint:
                "https://api.github.com/repos/example/project/actions/runs/42/attempts/1/logs"
                    .into(),
            acquisition: "native-https-download".into(),
            observer_revision: "synthetic-acceptance-fixture-v1".into(),
            capture_time_claim: "2026-01-01T00:00:00Z".into(),
            archive: log_archive::inspect(&archive).unwrap(),
            jobs_sha256: log_custody::digest(&serde_json::to_vec(&fixture_jobs()).unwrap()),
            non_emitting: vec![],
        };
        let expected = serde_json::to_value(&receipt).unwrap();
        let retained: Value = serde_json::from_slice(include_bytes!(
            "../../tests/fixtures/native-log-acceptance-receipt.json"
        ))
        .unwrap();
        assert_eq!(retained, expected);
    }

    struct Fixture {
        root: tempfile::TempDir,
        manifest: Manifest,
        source: FakeSource,
    }
    impl Fixture {
        fn new(step: bool) -> Self {
            use base64::Engine;
            let root = tempfile::tempdir().unwrap();
            let original = root.path().join("original.zip");
            fixture_archive(&original, true, false, false);
            let current = root.path().join("current.zip");
            fixture_archive(&current, step, false, true);
            let jobs = root.path().join("jobs.json");
            fs::write(&jobs, serde_json::to_vec(&fixture_jobs()).unwrap()).unwrap();
            let receipt = root.path().join("receipt.json");
            fs::write(
                &receipt,
                include_bytes!("../../tests/fixtures/native-log-acceptance-receipt.json"),
            )
            .unwrap();
            let signature = root.path().join("receipt.asc");
            fs::write(
                &signature,
                include_bytes!("../../tests/fixtures/native-log-acceptance-receipt.json.asc"),
            )
            .unwrap();
            let keyring = root.path().join("keyring.gpg");
            let encoded = include_str!("../../tests/fixtures/native-log-acceptance-key.asc")
                .lines()
                .filter(|line| !line.is_empty() && !line.starts_with('-') && !line.starts_with('='))
                .collect::<String>();
            fs::write(
                &keyring,
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .unwrap(),
            )
            .unwrap();
            write_json_new(
                &root.path().join("approval.json"),
                &Approval {
                    keyring,
                    fingerprint: "B74193E7547D0F6C870050C1BC430DBE2E1506A2".into(),
                    observer_revision: "synthetic-acceptance-fixture-v1".into(),
                },
            )
            .unwrap();
            Self {
                manifest: Manifest {
                    schema: "simit.native-log-manifest.v1".into(),
                    originals: vec![Original {
                        identity: fixture_identity(),
                        archive: original,
                        jobs,
                        observations: vec![],
                        supporting_records: vec![],
                        receipt: Some(receipt),
                        signature: Some(signature),
                        capture_record: None,
                        non_emitting_evidence: vec![],
                    }],
                },
                source: FakeSource {
                    current,
                    run_reads: 0,
                    mutation: Mutation::None,
                    interrupted: false,
                },
                root,
            }
        }
        fn args(&self, contract: Contract) -> Args {
            let path = self.root.path().join("manifest.json");
            fs::write(&path, serde_json::to_vec(&self.manifest).unwrap()).unwrap();
            Args {
                manifest: Some(path),
                legacy_root: None,
                source: None,
                base: None,
                checkout: None,
                pr_revision: None,
                results: self.root.path().join("results"),
                contract,
                observer_approval: Some(self.root.path().join("approval.json")),
                refresh_producers: true,
                refresh_archives: true,
                supporting_record: vec![],
            }
        }
        fn report(&self) -> Value {
            serde_json::from_slice(
                &fs::read(self.root.path().join("results/reconciliation.json")).unwrap(),
            )
            .unwrap()
        }
    }
    #[derive(Clone, Copy)]
    enum Mutation {
        None,
        Rerun,
        Failure,
        Source,
        Workflow,
        JobInventory,
        Unavailable,
    }
    struct FakeSource {
        current: PathBuf,
        run_reads: usize,
        mutation: Mutation,
        interrupted: bool,
    }
    impl EvidenceSource for FakeSource {
        fn metadata(
            &mut self,
            identity: &Identity,
            endpoint: &str,
            budget: &mut Budget,
        ) -> Result<Vec<u8>> {
            budget.request()?;
            if endpoint == "actions/runs/42" {
                self.run_reads += 1;
            }
            let final_phase = self.run_reads > 1;
            if final_phase && matches!(self.mutation, Mutation::Unavailable) {
                return Err(anyhow::anyhow!(ProviderUnavailable));
            }
            let value = match endpoint {
                "actions/runs/42" => {
                    json!({"id":42,"repository":{"id":1,"full_name":"example/project"},
                    "head_repository":{"id":1,"full_name":"example/project"},"run_attempt":if final_phase && matches!(self.mutation,Mutation::Rerun) {2}else{1},
                    "head_sha":identity.source,"event":"push","head_branch":"trunk","workflow_id":7,
                    "path":identity.workflow,"status":"completed","conclusion":if final_phase && matches!(self.mutation,Mutation::Failure) {"failure"}else{"success"}})
                }
                "actions/runs/42/attempts/1/jobs?per_page=100" => {
                    let mut jobs = fixture_jobs();
                    if final_phase && matches!(self.mutation, Mutation::JobInventory) {
                        jobs["jobs"][0]["steps"][0]["name"] = json!("changed");
                    }
                    jobs
                }
                "actions/workflows/7" => json!({"id":7,"path":identity.workflow,"state":"active"}),
                endpoint if endpoint.starts_with("contents/") => {
                    json!({"sha":if final_phase && matches!(self.mutation,Mutation::Workflow) {"e".repeat(40)}else{identity.workflow_blob.clone()}})
                }
                "git/ref/heads/trunk" => {
                    json!({"object":{"sha":if final_phase && matches!(self.mutation,Mutation::Source) {"e".repeat(40)}else{identity.source.clone()}}})
                }
                _ => anyhow::bail!("unexpected offline endpoint"),
            };
            let raw = serde_json::to_vec(&value)?;
            budget.charge_bytes(raw.len() as u64)?;
            Ok(raw)
        }
        fn capture(
            &mut self,
            _: &Identity,
            root: &Path,
            budget: &mut Budget,
        ) -> super::super::log_http::CaptureOutcome {
            let raw = fs::read(&self.current).unwrap();
            let path = root.join("42-fresh-native.zip");
            let captured = if self.interrupted { &raw[..32] } else { &raw };
            let result = budget
                .request()
                .and_then(|()| budget.request())
                .and_then(|()| budget.charge_bytes(captured.len() as u64));
            if let Err(error) = result {
                return super::super::log_http::CaptureOutcome {
                    path: None,
                    metadata: json!({}),
                    failure: Some(failure(&error, "api_request")),
                };
            }
            fs::write(&path, captured).unwrap();
            super::super::log_http::CaptureOutcome {
                path: Some(path),
                metadata: json!({"api_status":302,"download_status":200,"bytes":captured.len(),
                    "downloaded_sha256":log_custody::digest(captured),"body_complete":!self.interrupted}),
                failure: self
                    .interrupted
                    .then(|| json!({"stage":"download_body","code":"body_interrupted"})),
            }
        }
    }

    #[test]
    fn authenticated_full_refresh_accepts_equivalent_packaging_and_binds_both_phases() {
        let mut fixture = Fixture::new(true);
        let args = fixture.args(Contract::StrictFreshV1);
        run_with_source(args, Budget::default(), &mut fixture.source).unwrap();
        let report = fixture.report();
        assert_eq!(report["accepted"], true);
        let row = &report["originals"][0];
        assert_eq!(
            row["evaluations"][0]["provider_archive_state"],
            "equivalent_member_content"
        );
        for phase in ["fresh_metadata_references", "final_metadata_references"] {
            assert_eq!(row[phase].as_array().unwrap().len(), 5);
            for reference in row[phase].as_array().unwrap() {
                let raw = fs::read(
                    fixture
                        .root
                        .path()
                        .join("results")
                        .join(reference["name"].as_str().unwrap()),
                )
                .unwrap();
                assert_eq!(reference["sha256"], log_custody::digest(&raw));
                assert!(reference["observed_at_claim"].is_string());
                assert!(
                    reference["endpoint"]
                        .as_str()
                        .unwrap()
                        .starts_with("https://api.github.com/")
                );
            }
        }
    }

    #[test]
    fn omission_acceptance_requires_explicit_retained_contract_and_final_revocation() {
        for contract in [Contract::StrictFreshV1, Contract::RetainedNativeV1] {
            let mut fixture = Fixture::new(false);
            let args = fixture.args(contract);
            let result = run_with_source(args, Budget::default(), &mut fixture.source);
            assert_eq!(result.is_ok(), contract == Contract::RetainedNativeV1);
            assert_eq!(
                fixture.report()["accepted"],
                contract == Contract::RetainedNativeV1
            );
        }
        for mutation in [
            Mutation::Rerun,
            Mutation::Failure,
            Mutation::Source,
            Mutation::Workflow,
            Mutation::JobInventory,
            Mutation::Unavailable,
        ] {
            let mut fixture = Fixture::new(false);
            fixture.source.mutation = mutation;
            let args = fixture.args(Contract::RetainedNativeV1);
            assert!(run_with_source(args, Budget::default(), &mut fixture.source).is_err());
            let report = fixture.report();
            assert_eq!(report["accepted"], false);
            assert_eq!(report["originals"][0]["evaluations"][0]["accepted"], false);
            assert!(report["originals"][0]["eligibility_reason"].is_string());
        }
    }

    #[test]
    fn historical_conflict_revokes_current_evaluation_and_explains_row_rejection() {
        let mut fixture = Fixture::new(true);
        let conflict = fixture.root.path().join("conflict.zip");
        fixture_archive(&conflict, true, true, false);
        fixture.manifest.originals[0].observations.push(conflict);
        let args = fixture.args(Contract::RetainedNativeV1);
        assert!(run_with_source(args, Budget::default(), &mut fixture.source).is_err());
        let report = fixture.report();
        assert_eq!(report["accepted"], false);
        for result in report["originals"][0]["evaluations"].as_array().unwrap() {
            assert_eq!(result["accepted"], false);
            assert!(
                result["blockers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value.as_str().unwrap().contains("vetoes"))
            );
        }
    }

    #[test]
    fn interrupted_capture_retains_partial_metadata_without_acceptance() {
        let mut fixture = Fixture::new(true);
        fixture.source.interrupted = true;
        let args = fixture.args(Contract::RetainedNativeV1);
        assert!(run_with_source(args, Budget::default(), &mut fixture.source).is_err());
        let report = fixture.report();
        let capture = &report["originals"][0]["observations"][0];
        let partial = fs::read(fixture.root.path().join("results/42-fresh-native.zip")).unwrap();
        assert_eq!(
            capture["http_metadata"]["downloaded_sha256"],
            log_custody::digest(&partial)
        );
        assert_eq!(capture["http_metadata"]["body_complete"], false);
        assert_eq!(capture["partial_capture"], "42-fresh-native.zip");
        assert_eq!(report["accepted"], false);
    }

    #[test]
    fn exhausted_budget_retains_small_terminal_failure_record() {
        let mut fixture = Fixture::new(true);
        let args = fixture.args(Contract::StrictFreshV1);
        let budget = Budget::new(log_archive::Limits {
            bytes: 0,
            ..log_archive::Limits::default()
        })
        .unwrap();
        assert!(run_with_source(args, budget, &mut fixture.source).is_err());
        let terminal: Value = serde_json::from_slice(
            &fs::read(fixture.root.path().join("results/failure.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(terminal["accepted"], false);
        assert_eq!(terminal["failure"]["code"], "byte_budget_exhausted");
        assert_eq!(terminal["budget_usage"]["requests"], 0);
        assert!(
            !fixture
                .root
                .path()
                .join("results/reconciliation.json")
                .exists()
        );
    }

    #[test]
    fn pretty_serialization_limit_never_installs_partial_or_replaces_existing_result() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("report.json");
        assert!(write_json_bounded(&path, &json!({"large":"x".repeat(1024)}), 64).is_err());
        assert!(!path.exists());
        write_json_bounded(&path, &json!({"accepted":false}), 64).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(write_json_bounded(&path, &json!({"accepted":true}), 64).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn valid_archive_with_failed_storage_is_not_reported_as_invalid_zip() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("native.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        zip.start_file("job/system.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"native bytes").unwrap();
        zip.finish().unwrap();
        let value = observation(
            &path,
            None,
            &root.path().join("missing-storage"),
            &mut Budget::default(),
        );
        assert_eq!(value["archive_validation"], "verified");
        assert_eq!(value["state"], "unavailable");
        assert_eq!(value["failure"]["stage"], "storage_read_back");
        assert_eq!(value["failure"]["code"], "storage_io");
        assert_eq!(value["independent_read_back"], false);
    }
}
