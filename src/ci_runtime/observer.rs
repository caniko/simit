//! Read-only custody of exact original GitHub producers. Capture is not merge authority.
mod target;

use super::cancellation::Cancellation;
mod transport;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Cursor, Read, Write},
    path::Path,
    time::{Duration, Instant},
};
pub use transport::GitHubTransport;

const JSON_BYTES: u64 = 16 * 1024 * 1024;
/// Maximum JSON spec input, covering every admitted workflow/job-name inventory.
pub const MAX_SPEC_BYTES: u64 = JSON_BYTES;
const LOG_BYTES: u64 = 512 * 1024 * 1024;
const TOTAL_BYTES: u64 = 1536 * 1024 * 1024;
const MAX_WORKFLOWS: usize = 10;
const REQUEST_METADATA_BYTES: u64 = 4096;
// Provider responses/copies use the doubled body budget. Generated evidence is
// separately bounded: event/spec, three JSON snapshots per enrolled workflow,
// and one small diagnostic JSON file per admitted request.
const GENERATED_BYTES: u64 =
    (2 + 3 * MAX_WORKFLOWS as u64) * JSON_BYTES + MAX_REQUESTS as u64 * REQUEST_METADATA_BYTES;
const EVIDENCE_BYTES: u64 = 2 * TOTAL_BYTES + GENERATED_BYTES;
const RECEIPT_BYTES: u64 = 64 * 1024 * 1024;
// Final inventory excludes the receipt to avoid a self-referential hash, but
// the upload contains it. Expansion and download admission include that file.
const PACKET_BYTES: u64 = EVIDENCE_BYTES + RECEIPT_BYTES;
// Uploaded packets include request bodies and their captured archive copies.
// Keep an independent allowance for outer ZIP headers/deflate framing, and a
// separate request budget so metadata cannot truncate the admitted download.
const ARTIFACT_BYTES: u64 = PACKET_BYTES + 128 * 1024 * 1024;
const RETENTION_BYTES: u64 = ARTIFACT_BYTES + 256 * 1024 * 1024;
const MAX_REQUESTS: usize = 65536;
const MAX_PACKET_FILES: usize = 3 * MAX_REQUESTS + 10000;
const MAX_PAGES: u64 = 20;
const MAX_ATTEMPTS: u64 = 5;

/// Enrollment supplied by repository policy, never inferred from captured receipts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// `simit.original-observer-spec.v1`.
    pub schema: String,
    /// GitHub owner/repository.
    pub repository: String,
    /// Exact workflow paths and complete required job-name inventories.
    pub workflows: BTreeMap<String, Vec<String>>,
    /// One aggregate discovery/poll/capture deadline; reserve upload time outside it.
    pub timeout_seconds: u64,
    /// Nonterminal/discovery polling interval.
    pub poll_seconds: u64,
}

impl Spec {
    /// Reject unsupported enrollment before creating evidence or contacting a provider.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == "simit.original-observer-spec.v1",
            "unknown observer schema"
        );
        let parts: Vec<_> = self.repository.split('/').collect();
        ensure!(
            parts.len() == 2
                && parts.iter().all(|s| !s.is_empty()
                    && s.len() <= 100
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))),
            "invalid repository"
        );
        ensure!(
            (1..=MAX_WORKFLOWS).contains(&self.workflows.len()),
            "invalid workflow inventory"
        );
        for (path, jobs) in &self.workflows {
            let name = path
                .strip_prefix(".github/workflows/")
                .context("invalid workflow path")?;
            ensure!(
                !name.is_empty()
                    && name.len() <= 200
                    && !name.starts_with('.')
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                    && (name.ends_with(".yaml") || name.ends_with(".yml")),
                "invalid workflow path"
            );
            ensure!(
                !jobs.is_empty()
                    && jobs.len() <= 256
                    && jobs.iter().collect::<BTreeSet<_>>().len() == jobs.len()
                    && jobs.iter().all(|s| !s.is_empty()
                        && s.len() <= 512
                        && !s.chars().any(char::is_control)),
                "invalid required jobs"
            );
        }
        ensure!(
            (1..=20700).contains(&self.timeout_seconds) && (1..=300).contains(&self.poll_seconds),
            "invalid observer time budget"
        );
        // Cover the entire accepted schedule, including maximum discovery pages,
        // every pending workflow, capture pages and all transient retry attempts.
        // Discovery and polling share the deadline; counting both per interval
        // deliberately overestimates their combined upper bound.
        let rounds = self.timeout_seconds.div_ceil(self.poll_seconds) + 1;
        let capture = self
            .workflows
            .values()
            .map(|jobs| (jobs.len() as u64).div_ceil(100) + 4)
            .sum::<u64>();
        let required = MAX_ATTEMPTS
            * (rounds * (MAX_PAGES + self.workflows.len() as u64) + capture + MAX_PAGES);
        ensure!(
            required <= MAX_REQUESTS as u64,
            "polling configuration exceeds aggregate request allowance"
        );
        Ok(())
    }
}

/// Observer identity resolved independently from the provider event/environment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    /// Immutable provider repository id from the native event.
    pub repository_id: u64,
    /// Exact branch or PR source head.
    pub head: String,
    /// Only branch push or pull_request.
    pub event: String,
    /// Original observer run.
    pub run_id: u64,
    /// Original observer attempt, exactly one.
    pub run_attempt: u64,
    /// Native workflow ref, including its repository and ref.
    pub workflow_ref: String,
    /// Native observer workflow commit.
    pub workflow_sha: String,
    /// Exact native event bytes (UTF-8 JSON), retained without normalization.
    pub event_json: String,
}

fn oid(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Identity {
    /// Resolve and validate the hosted observer environment.
    pub fn from_environment(spec: &Spec) -> Result<Self> {
        ensure!(
            std::env::var("RUNNER_ENVIRONMENT")? == "github-hosted",
            "hosted observer required"
        );
        ensure!(
            std::env::var("GITHUB_REPOSITORY")? == spec.repository,
            "observer repository changed"
        );
        let event_json = String::from_utf8(read_bytes(
            Path::new(&std::env::var("GITHUB_EVENT_PATH")?),
            JSON_BYTES,
        )?)?;
        let payload: Value = serde_json::from_str(&event_json)?;
        let event = std::env::var("GITHUB_EVENT_NAME")?;
        let head = if event == "pull_request" {
            payload["pull_request"]["head"]["sha"]
                .as_str()
                .context("missing PR head")?
                .to_owned()
        } else {
            std::env::var("GITHUB_SHA")?
        };
        let identity = Self {
            repository_id: payload["repository"]["id"]
                .as_u64()
                .context("missing repository id")?,
            head,
            event,
            run_id: std::env::var("GITHUB_RUN_ID")?.parse()?,
            run_attempt: std::env::var("GITHUB_RUN_ATTEMPT")?.parse()?,
            workflow_ref: std::env::var("GITHUB_WORKFLOW_REF")?,
            workflow_sha: std::env::var("GITHUB_WORKFLOW_SHA")?,
            event_json,
        };
        identity.validate(spec)?;
        Ok(identity)
    }
    /// Validate exact original observer/event identity.
    pub fn validate(&self, spec: &Spec) -> Result<()> {
        spec.validate()?;
        ensure!(
            oid(&self.head) && oid(&self.workflow_sha) && self.run_id > 0 && self.run_attempt == 1,
            "invalid original observer identity"
        );
        ensure!(
            self.workflow_ref
                .starts_with(&format!("{}/.github/workflows/", spec.repository))
                && self.workflow_ref.contains('@')
                && self.workflow_ref.len() <= 2048
                && !self.workflow_ref.chars().any(char::is_control),
            "invalid observer workflow ref"
        );
        let observer_path = self
            .workflow_ref
            .strip_prefix(&format!("{}/", spec.repository))
            .and_then(|reference| reference.split_once('@'))
            .context("invalid observer workflow ref")?
            .0;
        ensure!(
            !spec.workflows.contains_key(observer_path),
            "active observer workflow cannot be enrolled as a producer"
        );
        ensure!(
            self.event_json.len() as u64 <= JSON_BYTES,
            "event payload exceeds budget"
        );
        let payload: Value = serde_json::from_str(&self.event_json)?;
        ensure!(
            self.repository_id > 0
                && payload["repository"]["id"].as_u64() == Some(self.repository_id)
                && payload["repository"]["full_name"] == spec.repository,
            "native event repository changed"
        );
        match self.event.as_str() {
            "push" => ensure!(
                payload["after"] == self.head
                    && self.workflow_sha == self.head
                    && payload["deleted"] == false
                    && payload["ref"]
                        .as_str()
                        .is_some_and(|r| r.starts_with("refs/heads/")),
                "push source changed"
            ),
            "pull_request" => ensure!(
                payload["pull_request"]["head"]["sha"] == self.head,
                "PR source changed"
            ),
            _ => bail!("unsupported observer event"),
        }
        Ok(())
    }
}

/// One bounded GET response, including interrupted bytes and sanitized errors.
pub struct Response {
    /// HTTP status, zero for a transport failure before response headers.
    pub status: u16,
    /// Actual received bytes, including partial or unsuccessful response bodies.
    pub body: Vec<u8>,
    /// Provider retry-after in seconds, when present.
    pub retry_after: Option<u64>,
    /// Sanitized transport class; contains neither tokens nor signed URLs.
    pub transport_error: Option<String>,
}

/// Injectable read-only fixed-provider boundary. Implementations must honor byte/time limits.
pub trait Transport {
    /// GET a repository-relative endpoint; no mutation or producer execution is exposed.
    fn get(
        &mut self,
        endpoint: &str,
        maximum: u64,
        timeout: Duration,
        cancellation: &Cancellation,
    ) -> Result<Response>;
}

/// Injectable monotonic clock for one aggregate deadline, including retries and sleeps.
pub trait Clock {
    /// Elapsed time since this observation's clock was created.
    fn elapsed(&self) -> Duration;
    /// Wait while honoring cancellation.
    fn sleep(&mut self, duration: Duration, cancellation: &Cancellation) -> Result<()>;
}

/// Production monotonic clock.
pub struct MonotonicClock(Instant);
impl Default for MonotonicClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl Clock for MonotonicClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&mut self, duration: Duration, cancellation: &Cancellation) -> Result<()> {
        let end = Instant::now() + duration;
        while Instant::now() < end {
            cancellation.check()?;
            std::thread::sleep(
                end.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(100)),
            );
        }
        cancellation.check()
    }
}

/// Captured successful original producer, including raw archive and member identities.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Producer {
    /// Immutable original run id.
    pub run_id: u64,
    /// Enrolled workflow path.
    pub workflow: String,
    /// Successful terminal provider snapshot.
    pub terminal: Value,
    /// SHA-256 of the complete provider ZIP bytes.
    pub raw_logs_sha256: String,
    /// CRC-checked, bounded provider log member hashes.
    pub log_members: BTreeMap<String, String>,
}

/// Evidence only. Provider retention, workflow coverage and merge policy remain separate gates.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    /// `simit.original-custody.v1`.
    pub schema: String,
    /// Always false: capture alone is not qualification authority.
    pub qualified: bool,
    /// True only after all enrolled exact originals were successfully captured and revalidated.
    pub capture_complete: bool,
    /// Independent enrollment retained for subsequent comparison.
    pub spec: Spec,
    /// Independent observer identity retained for subsequent comparison.
    pub identity: Identity,
    /// Captured producer rows.
    pub producers: Vec<Producer>,
    /// Explicit failure when observation is incomplete.
    pub error: Option<String>,
    /// SHA-256 of all retained evidence files, excluding this receipt.
    pub members: BTreeMap<String, String>,
}

fn digest(raw: &[u8]) -> String {
    hex::encode(Sha256::digest(raw))
}
fn read_bytes(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(maximum + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= maximum, "file exceeds byte limit");
    Ok(bytes)
}
struct JsonBudget<W> {
    writer: W,
    remaining: u64,
}
impl<W: Write> Write for JsonBudget<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(std::io::Error::other("serialized JSON exceeds byte limit"));
        }
        let count = self.writer.write(bytes)?;
        self.remaining -= count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}
fn write_json_to(file: impl Write, value: &impl Serialize, maximum: u64) -> Result<()> {
    let mut writer = JsonBudget {
        writer: std::io::BufWriter::new(file),
        remaining: maximum,
    };
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}
fn write_json_bounded(path: &Path, value: &impl Serialize, maximum: u64) -> Result<()> {
    write_json_to(
        OpenOptions::new().write(true).create_new(true).open(path)?,
        value,
        maximum,
    )
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    write_json_bounded(path, value, JSON_BYTES)
}
fn save_receipt(root: &Path, receipt: &Receipt) -> Result<()> {
    save_receipt_with_limit(root, receipt, RECEIPT_BYTES)
}
fn save_receipt_with_limit(root: &Path, receipt: &Receipt, maximum: u64) -> Result<()> {
    // NamedTempFile owns only this new write. A failed serialization removes its
    // partial temporary file, preserving the prior receipt and allowing the
    // final non-accepting receipt to publish through the same atomic path.
    let mut temporary = tempfile::Builder::new()
        .prefix(".receipt-next-")
        .tempfile_in(root)?;
    write_json_to(temporary.as_file_mut(), receipt, maximum)?;
    temporary.persist(root.join("receipt.json"))?;
    Ok(())
}

fn append_producer(
    root: &Path,
    receipt: &mut Receipt,
    producer: Producer,
    maximum: u64,
) -> Result<()> {
    receipt.producers.push(producer);
    if let Err(error) = save_receipt_with_limit(root, receipt, maximum) {
        // Full archives and maps have already been retained outside the receipt.
        // Roll back only this duplicated row so failure publication can proceed.
        receipt.producers.pop();
        return Err(error.context(
            "aggregate producer receipt could not be published; original capture files retained",
        ));
    }
    Ok(())
}

fn publish_final_receipt(root: &Path, receipt: &mut Receipt, maximum: u64) -> Result<()> {
    if let Err(error) = save_receipt_with_limit(root, receipt, maximum) {
        // Final inventory/failure text can grow beyond a previously admitted
        // partial receipt. Publish an explicit non-accepting recovery record,
        // never leave its older pending version looking like the final outcome.
        receipt.capture_complete = false;
        receipt.qualified = false;
        let earlier = receipt
            .error
            .as_deref()
            .unwrap_or("capture had no prior error")
            .chars()
            .take(2048)
            .collect::<String>();
        let detail = error.to_string().chars().take(2048).collect::<String>();
        receipt.error = Some(format!(
            "final receipt publication failed: {detail}; {earlier}; full captured evidence retained outside this incomplete receipt"
        ));
        for producer in &mut receipt.producers {
            producer.log_members.clear();
        }
        if save_receipt_with_limit(root, receipt, maximum).is_err() {
            // The complete inventory is still present in the original packet;
            // an incomplete receipt cannot certify it through a partial map.
            receipt.members.clear();
            save_receipt_with_limit(root, receipt, maximum)?;
        }
        return Err(error.context(
            "final receipt exceeded its publication envelope; explicit failure receipt retained",
        ));
    }
    Ok(())
}

struct Observer<'a> {
    spec: &'a Spec,
    root: &'a Path,
    transport: &'a mut dyn Transport,
    clock: &'a mut dyn Clock,
    cancellation: &'a Cancellation,
    requests: usize,
    bytes: u64,
    byte_budget: u64,
}
impl Observer<'_> {
    fn remaining(&self) -> Result<Duration> {
        self.cancellation.check()?;
        let remaining = Duration::from_secs(self.spec.timeout_seconds)
            .checked_sub(self.clock.elapsed())
            .context("aggregate observer deadline exceeded")?;
        ensure!(!remaining.is_zero(), "aggregate observer deadline exceeded");
        Ok(remaining)
    }
    fn sleep(&mut self, seconds: u64) -> Result<()> {
        let remaining = self.remaining()?;
        self.clock.sleep(
            Duration::from_secs(seconds).min(remaining),
            self.cancellation,
        )?;
        self.remaining()?;
        Ok(())
    }
    fn get(&mut self, suffix: &str, maximum: u64) -> Result<Vec<u8>> {
        for attempt in 0..MAX_ATTEMPTS {
            let remaining = self.remaining()?;
            let timeout = if suffix.ends_with("/logs") || suffix.ends_with("/zip") {
                remaining
            } else {
                remaining.min(Duration::from_secs(90))
            };
            self.requests += 1;
            ensure!(
                self.requests <= MAX_REQUESTS,
                "observer request inventory exceeded"
            );
            let maximum = maximum.min(self.byte_budget.saturating_sub(self.bytes));
            ensure!(maximum > 0, "observer byte budget exhausted");
            let endpoint = format!("repos/{}/{suffix}", self.spec.repository);
            let response = self
                .transport
                .get(&endpoint, maximum, timeout, self.cancellation)?;
            ensure!(
                response.body.len() as u64 <= maximum,
                "provider exceeded byte limit"
            );
            self.bytes += response.body.len() as u64;
            let name = format!("requests/{:04}", self.requests);
            fs::write(self.root.join(format!("{name}.body")), &response.body)?;
            write_json_bounded(
                &self.root.join(format!("{name}.json")),
                &json!({"endpoint":endpoint,"status":response.status,"transport_error":response.transport_error,"retry_after":response.retry_after,"attempt":attempt+1}),
                REQUEST_METADATA_BYTES,
            )?;
            self.remaining()?;
            if response.status == 200 && response.transport_error.is_none() {
                return Ok(response.body);
            }
            let transient_status = matches!(response.status, 408 | 429 | 500 | 502 | 503 | 504)
                || (response.status == 403 && response.retry_after.is_some());
            let transient = match response.transport_error.as_deref() {
                None | Some("byte-limit") => transient_status,
                Some("timeout" | "connect" | "body-read") => {
                    transient_status || matches!(response.status, 0 | 200)
                }
                _ => false,
            };
            ensure!(
                transient && attempt + 1 < MAX_ATTEMPTS,
                "provider GET failed: status {} ({})",
                response.status,
                response.transport_error.as_deref().unwrap_or("HTTP")
            );
            // Never retry before the provider's rate-limit window. The shared
            // deadline bounds even an unusually large Retry-After value.
            self.sleep(response.retry_after.unwrap_or(1 << attempt).max(1))?;
        }
        unreachable!("retry loop always returns or rejects")
    }
    fn json(&mut self, suffix: &str) -> Result<Value> {
        Ok(serde_json::from_slice(&self.get(suffix, JSON_BYTES)?)?)
    }
    fn pages(&mut self, suffix: &str, key: &str) -> Result<Vec<Value>> {
        let mut rows = Vec::new();
        let mut total = None;
        for page in 1..=MAX_PAGES {
            let separator = if suffix.contains('?') { '&' } else { '?' };
            let response = self.json(&format!("{suffix}{separator}per_page=100&page={page}"))?;
            let count = response["total_count"]
                .as_u64()
                .context("missing provider total_count")?;
            ensure!(
                count <= 2000 && total.is_none_or(|n| n == count),
                "provider pagination inventory changed"
            );
            total = Some(count);
            let values = response[key]
                .as_array()
                .context("missing provider page inventory")?;
            ensure!(values.len() <= 100, "oversized provider page");
            rows.extend(values.iter().cloned());
            ensure!(
                rows.len() as u64 <= count,
                "duplicate or oversized provider inventory"
            );
            if rows.len() as u64 == count {
                return Ok(rows);
            }
            ensure!(values.len() == 100, "incomplete provider pagination");
        }
        bail!("provider pagination exceeded bound")
    }
}

fn validate_run(
    run: &Value,
    spec: &Spec,
    identity: &Identity,
    id: u64,
    workflow: &str,
) -> Result<()> {
    ensure!(
        run["id"].as_u64() == Some(id)
            && id > 0
            && run["head_sha"] == identity.head
            && run["event"] == identity.event
            && run["run_attempt"] == 1
            && provider_workflow(run)? == workflow
            && run["repository"]["full_name"] == spec.repository,
        "producer identity changed"
    );
    ensure!(
        run["repository"]["id"].as_u64() == Some(identity.repository_id),
        "producer repository id changed"
    );
    target::validate(run, spec, identity)?;
    Ok(())
}

fn provider_workflow(run: &Value) -> Result<&str> {
    let path = run["path"]
        .as_str()
        .context("missing producer workflow path")?;
    if let Some((path, reference)) = path.split_once('@') {
        ensure!(
            target::workflow_reference(reference),
            "invalid producer workflow ref suffix"
        );
        Ok(path)
    } else {
        Ok(path)
    }
}

fn select_runs(
    runs: Vec<Value>,
    spec: &Spec,
    identity: &Identity,
) -> Result<BTreeMap<String, u64>> {
    let mut selected = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for run in runs {
        // Inspect only the path portion for enrollment. Suffix syntax and all
        // other producer admission apply after target filtering.
        let path = run["path"]
            .as_str()
            .context("missing producer workflow path")?;
        let workflow = path.split_once('@').map_or(path, |(path, _)| path);
        if spec.workflows.contains_key(workflow) && target::matches(&run, identity)? {
            let id = run["id"].as_u64().context("missing producer run id")?;
            validate_run(&run, spec, identity, id, workflow)?;
            ensure!(
                selected.insert(workflow.to_owned(), id).is_none() && ids.insert(id),
                "duplicate original producer"
            );
        }
    }
    Ok(selected)
}

fn compact_run(run: &Value) -> Value {
    // Keep the validated wire identity in the receipt, not arbitrary ignored
    // provider JSON. Full snapshots remain byte-bound in the retained packet.
    let mut value = serde_json::Map::new();
    for key in [
        "id",
        "head_sha",
        "event",
        "run_attempt",
        "path",
        "status",
        "conclusion",
        "head_branch",
    ] {
        value.insert(key.into(), run[key].clone());
    }
    value.insert(
        "repository".into(),
        json!({"id":run["repository"]["id"],"full_name":run["repository"]["full_name"]}),
    );
    value.insert("head_repository".into(),if run["event"]=="push" {
        json!({"id":run["head_repository"]["id"],"full_name":run["head_repository"]["full_name"]})
    } else {json!({"id":run["head_repository"]["id"]})});
    if let Some(associations) = run["pull_requests"]
        .as_array()
        .filter(|_| run["event"] == "pull_request")
    {
        value.insert("pull_requests".into(),json!(associations.iter().map(|pr|json!({"number":pr["number"],
            "head":{"ref":pr["head"]["ref"],"sha":pr["head"]["sha"],"repo":{"id":pr["head"]["repo"]["id"]}},
            "base":{"ref":pr["base"]["ref"],"sha":pr["base"]["sha"],"repo":{"id":pr["base"]["repo"]["id"]}}
        })).collect::<Vec<_>>()));
    }
    Value::Object(value)
}
fn successful_run(run: &Value) -> Result<()> {
    ensure!(
        run["status"] == "completed" && run["conclusion"] == "success",
        "producer did not complete successfully"
    );
    Ok(())
}
fn validate_jobs(
    jobs: &[Value],
    spec: &Spec,
    identity: &Identity,
    id: u64,
    workflow: &str,
) -> Result<()> {
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for job in jobs {
        let job_id = job["id"].as_u64().context("missing job id")?;
        ensure!(
            job_id > 0
                && ids.insert(job_id)
                && job["run_id"] == id
                && job["run_attempt"] == 1
                && job["head_sha"] == identity.head,
            "job identity changed or duplicated"
        );
        ensure!(
            names.insert(job["name"].as_str().context("missing job name")?.to_owned()),
            "duplicate job name"
        );
        successful_run(job)?;
        let steps = job["steps"].as_array().context("missing native steps")?;
        ensure!(
            !steps.is_empty() && steps.len() <= 1000,
            "missing or oversized native steps"
        );
        let mut numbers = BTreeSet::new();
        for step in steps {
            ensure!(
                step["number"]
                    .as_u64()
                    .is_some_and(|n| n > 0 && numbers.insert(n)),
                "duplicate native step"
            );
            successful_run(step)?;
        }
    }
    ensure!(
        names == spec.workflows[workflow].iter().cloned().collect(),
        "required job inventory differs"
    );
    Ok(())
}

fn log_members(
    raw: &[u8],
    mut check: impl FnMut() -> Result<()>,
) -> Result<BTreeMap<String, String>> {
    archive_members(raw, LOG_BYTES, 10000, &mut check)
}
fn archive_members(
    raw: &[u8],
    maximum: u64,
    maximum_files: usize,
    mut check: impl FnMut() -> Result<()>,
) -> Result<BTreeMap<String, String>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(raw))?;
    ensure!(
        archive.len() <= maximum_files,
        "ZIP member inventory exceeded"
    );
    // zip's filename index can hide duplicate central-directory names. Compare
    // against the original EOCD inventory before trusting indexed enumeration.
    let end = raw
        .windows(4)
        .rposition(|bytes| bytes == b"PK\x05\x06")
        .context("missing ZIP directory footer")?;
    let footer = raw.get(end..end + 22).context("truncated ZIP footer")?;
    let mut entries = u16::from_le_bytes([footer[10], footer[11]]) as usize;
    if entries == u16::MAX as usize
        && end
            .checked_sub(20)
            .and_then(|offset| raw.get(offset..offset + 4))
            == Some(b"PK\x06\x07")
    {
        let locator_offset = end
            .checked_sub(20)
            .context("missing ZIP64 directory locator")?;
        let locator = &raw[locator_offset..end];
        ensure!(
            locator[..4] == *b"PK\x06\x07"
                && locator[4..8] == [0; 4]
                && locator[16..20] == [1, 0, 0, 0],
            "unsupported ZIP64 disks"
        );
        let offset = usize::try_from(u64::from_le_bytes(locator[8..16].try_into()?))?;
        let directory = raw
            .get(offset..offset.checked_add(56).context("ZIP64 directory overflow")?)
            .context("truncated ZIP64 directory")?;
        let size = usize::try_from(u64::from_le_bytes(directory[4..12].try_into()?))?;
        ensure!(
            directory[..4] == *b"PK\x06\x06"
                && size >= 44
                && offset.checked_add(12).and_then(|n| n.checked_add(size)) == Some(locator_offset)
                && directory[16..24] == [0; 8]
                && directory[24..32] == directory[32..40],
            "unsupported ZIP64 directory"
        );
        entries = usize::try_from(u64::from_le_bytes(directory[32..40].try_into()?))?;
    }
    let comment = u16::from_le_bytes([footer[20], footer[21]]) as usize;
    ensure!(
        end + 22 + comment == raw.len()
            && entries == archive.len()
            && footer[4..8] == [0; 4]
            && footer[8..10] == footer[10..12],
        "duplicate, multidisk or unsupported ZIP directory"
    );
    let mut names = BTreeSet::new();
    let mut result = BTreeMap::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        check()?;
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_owned();
        ensure!(
            names.insert(name.clone()) && entry.enclosed_name().is_some() && !name.contains('\\'),
            "duplicate or unsafe log ZIP name"
        );
        ensure!(
            entry.unix_mode().is_none_or(|m| m & 0o170000 != 0o120000),
            "log ZIP symlink rejected"
        );
        if entry.is_dir() {
            continue;
        }
        let mut hash = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            check()?;
            let count = entry.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total += count as u64;
            ensure!(total <= maximum, "log ZIP expansion budget exceeded");
            hash.update(&buffer[..count]);
        }
        result.insert(name, hex::encode(hash.finalize()));
    }
    ensure!(
        !result.is_empty(),
        "successful producer has no provider log files"
    );
    Ok(result)
}

/// Observe with the native transport and a caller-owned cancellation token.
pub fn observe(
    spec: &Spec,
    identity: &Identity,
    root: &Path,
    transport: &mut dyn Transport,
    cancellation: &Cancellation,
) -> Result<Receipt> {
    observe_with(
        spec,
        identity,
        root,
        transport,
        &mut MonotonicClock::default(),
        cancellation,
    )
}

/// Observe all enrolled originals under one deadline, retaining an explicit failed receipt on error.
pub fn observe_with(
    spec: &Spec,
    identity: &Identity,
    root: &Path,
    transport: &mut dyn Transport,
    clock: &mut dyn Clock,
    cancellation: &Cancellation,
) -> Result<Receipt> {
    identity.validate(spec)?;
    fs::create_dir(root).context("observer requires a fresh evidence directory")?;
    fs::create_dir(root.join("requests"))?;
    let mut receipt = Receipt {
        schema: "simit.original-custody.v1".into(),
        qualified: false,
        capture_complete: false,
        spec: spec.clone(),
        identity: identity.clone(),
        producers: Vec::new(),
        error: None,
        members: BTreeMap::new(),
    };
    save_receipt(root, &receipt)?;
    let mut observer = Observer {
        spec,
        root,
        transport,
        clock,
        cancellation,
        requests: 0,
        bytes: 0,
        byte_budget: TOTAL_BYTES,
    };
    let outcome = (|| {
        fs::write(root.join("event.json"), identity.event_json.as_bytes())?;
        write_json(&root.join("spec.json"), spec)?;
        let selected = loop {
            let runs = observer.pages(
                &format!(
                    "actions/runs?head_sha={}&event={}",
                    identity.head, identity.event
                ),
                "workflow_runs",
            )?;
            let selected = select_runs(runs, spec, identity)?;
            if selected.len() == spec.workflows.len() {
                break selected;
            }
            observer.sleep(spec.poll_seconds)?;
        };
        let mut pending = selected.clone();
        while !pending.is_empty() {
            for (workflow, id) in pending.clone() {
                let run = observer.json(&format!("actions/runs/{id}"))?;
                validate_run(&run, spec, identity, id, &workflow)?;
                if run["status"] != "completed" {
                    ensure!(
                        matches!(
                            run["status"].as_str(),
                            Some("queued" | "in_progress" | "waiting" | "requested" | "pending")
                        ),
                        "unknown producer status"
                    );
                    continue;
                }
                let folder = root.join(id.to_string());
                fs::create_dir(&folder)?;
                write_json(&folder.join("terminal.json"), &run)?;
                let jobs = observer.pages(&format!("actions/runs/{id}/attempts/1/jobs"), "jobs")?;
                write_json(&folder.join("jobs.json"), &jobs)
                    .context("aggregate jobs exceed the retention byte limit")?;
                let raw = observer.get(&format!("actions/runs/{id}/attempts/1/logs"), LOG_BYTES)?;
                fs::write(folder.join("provider-logs.zip"), &raw)?;
                // Retain failed/cancelled originals too; unsuccessful producer
                // execution must not erase its raw diagnostic archive.
                let members = log_members(&raw, || observer.remaining().map(|_| ()))?;
                write_json(&folder.join("provider-log-members.json"), &members)?;
                successful_run(&run)?;
                validate_jobs(&jobs, spec, identity, id, &workflow)?;
                let after = observer.json(&format!("actions/runs/{id}"))?;
                validate_run(&after, spec, identity, id, &workflow)?;
                successful_run(&after)?;
                append_producer(
                    root,
                    &mut receipt,
                    Producer {
                        run_id: id,
                        workflow: workflow.clone(),
                        terminal: compact_run(&after),
                        raw_logs_sha256: digest(&raw),
                        log_members: members,
                    },
                    RECEIPT_BYTES,
                )?;
                pending.remove(&workflow);
            }
            if !pending.is_empty() {
                observer.sleep(spec.poll_seconds)?;
            }
        }
        for producer in &receipt.producers {
            let fresh = observer.json(&format!("actions/runs/{}", producer.run_id))?;
            validate_run(&fresh, spec, identity, producer.run_id, &producer.workflow)?;
            successful_run(&fresh)?;
        }
        let fresh = observer.pages(
            &format!(
                "actions/runs?head_sha={}&event={}",
                identity.head, identity.event
            ),
            "workflow_runs",
        )?;
        let fresh_selected = select_runs(fresh, spec, identity)?;
        ensure!(
            fresh_selected == selected,
            "original producer selection changed"
        );
        observer.remaining()?;
        Ok(())
    })();
    receipt.error = outcome
        .as_ref()
        .err()
        .map(|error: &anyhow::Error| error.to_string());
    // Finalization has its own finite reserve. It runs even after deadline/cancellation
    // so the uploaded packet explicitly remains incomplete instead of lacking a receipt.
    let finalized = inventory(root);
    if let Ok(members) = finalized.as_ref() {
        receipt.members = members.clone();
    }
    if let Err(error) = &finalized {
        receipt.error = Some(format!("evidence finalization failed: {error}"));
    }
    receipt.capture_complete = outcome.is_ok() && finalized.is_ok() && cancellation.check().is_ok();
    if !receipt.capture_complete && receipt.error.is_none() {
        receipt.error = Some("observer cancelled before publication".into());
    }
    publish_final_receipt(root, &mut receipt, RECEIPT_BYTES)?;
    outcome?;
    finalized?;
    ensure!(
        receipt.capture_complete,
        "observer cancelled before publication"
    );
    Ok(receipt)
}

fn inventory(root: &Path) -> Result<BTreeMap<String, String>> {
    let start = Instant::now();
    let mut pending = vec![root.to_owned()];
    let mut result = BTreeMap::new();
    let mut total = 0u64;
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            ensure!(!kind.is_symlink(), "evidence symlink rejected");
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            ensure!(kind.is_file(), "special evidence file rejected");
            let relative = path
                .strip_prefix(root)?
                .components()
                .map(|component| {
                    let name = component
                        .as_os_str()
                        .to_str()
                        .context("non-UTF8 evidence path")?;
                    ensure!(!name.contains('\\'), "nonportable evidence path rejected");
                    Ok(name)
                })
                .collect::<Result<Vec<_>>>()?
                .join("/");
            if relative == "receipt.json" {
                continue;
            }
            ensure!(
                result.len() < MAX_PACKET_FILES,
                "evidence file inventory exceeded"
            );
            let mut file = File::open(path)?;
            let mut hash = Sha256::new();
            let mut buffer = [0; 65536];
            loop {
                ensure!(
                    start.elapsed() < Duration::from_secs(120),
                    "finalization deadline exceeded"
                );
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                total += count as u64;
                ensure!(total <= EVIDENCE_BYTES, "finalization byte budget exceeded");
                hash.update(&buffer[..count]);
            }
            result.insert(relative, hex::encode(hash.finalize()));
        }
    }
    Ok(result)
}

/// Verify uploaded provider custody against independently supplied source enrollment.
/// This checks capture, content and retention; it does not grant merge authority.
pub fn verify_retention(
    spec: &Spec,
    identity: &Identity,
    root: &Path,
    artifact_id: u64,
    upload_digest: &str,
    transport: &mut dyn Transport,
    cancellation: &Cancellation,
) -> Result<Value> {
    identity.validate(spec)?;
    ensure!(artifact_id > 0, "invalid uploaded artifact id");
    let upload_digest = upload_digest
        .strip_prefix("sha256:")
        .unwrap_or(upload_digest);
    ensure!(
        upload_digest.len() == 64
            && upload_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid upload digest"
    );
    let receipt_bytes = read_bytes(&root.join("receipt.json"), RECEIPT_BYTES)?;
    let receipt: Receipt = serde_json::from_slice(&receipt_bytes)?;
    ensure!(
        receipt.schema == "simit.original-custody.v1"
            && !receipt.qualified
            && receipt.capture_complete
            && receipt.error.is_none()
            && receipt.spec == *spec
            && receipt.identity == *identity,
        "incomplete or substituted custody receipt"
    );
    ensure!(
        inventory(root)? == receipt.members,
        "custody evidence inventory differs"
    );
    ensure!(
        read_bytes(&root.join("event.json"), JSON_BYTES)? == identity.event_json.as_bytes(),
        "event bytes changed"
    );
    let mut enrolled = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for producer in &receipt.producers {
        ensure!(
            spec.workflows.contains_key(&producer.workflow)
                && enrolled.insert(producer.workflow.clone())
                && ids.insert(producer.run_id),
            "duplicate or unenrolled producer"
        );
        validate_run(
            &producer.terminal,
            spec,
            identity,
            producer.run_id,
            &producer.workflow,
        )?;
        successful_run(&producer.terminal)?;
        let folder = root.join(producer.run_id.to_string());
        let terminal: Value =
            serde_json::from_slice(&read_bytes(&folder.join("terminal.json"), JSON_BYTES)?)?;
        validate_run(
            &terminal,
            spec,
            identity,
            producer.run_id,
            &producer.workflow,
        )?;
        successful_run(&terminal)?;
        let jobs: Vec<Value> =
            serde_json::from_slice(&read_bytes(&folder.join("jobs.json"), JSON_BYTES)?)?;
        validate_jobs(&jobs, spec, identity, producer.run_id, &producer.workflow)?;
        let raw = read_bytes(&folder.join("provider-logs.zip"), LOG_BYTES)?;
        ensure!(
            digest(&raw) == producer.raw_logs_sha256
                && log_members(&raw, || cancellation.check())? == producer.log_members,
            "provider archive differs"
        );
    }
    ensure!(
        enrolled == spec.workflows.keys().cloned().collect(),
        "missing enrolled producers"
    );
    // Verification uses a fresh, short aggregate read budget and fresh evidence,
    // outside the uploaded packet, so no uploaded bytes are silently replaced.
    let mut name = root
        .file_name()
        .context("custody packet must have a directory name")?
        .to_os_string();
    name.push(".retention");
    let retained = root.with_file_name(name);
    fs::create_dir(&retained).context("retention evidence requires a fresh sibling directory")?;
    fs::create_dir(retained.join("requests"))?;
    let mut verify_spec = spec.clone();
    verify_spec.timeout_seconds = 300;
    let mut clock = MonotonicClock::default();
    let mut observer = Observer {
        spec: &verify_spec,
        root: &retained,
        transport,
        clock: &mut clock,
        cancellation,
        requests: 0,
        bytes: 0,
        byte_budget: RETENTION_BYTES,
    };
    let artifact = observer.json(&format!("actions/artifacts/{artifact_id}"))?;
    validate_artifact(&artifact, identity, artifact_id, upload_digest)?;
    let raw = observer.get(
        &format!("actions/artifacts/{artifact_id}/zip"),
        ARTIFACT_BYTES,
    )?;
    ensure!(
        digest(&raw) == upload_digest,
        "uploaded provider ZIP digest differs"
    );
    let mut expected = receipt.members.clone();
    expected.insert("receipt.json".into(), digest(&receipt_bytes));
    ensure!(
        archive_members(&raw, PACKET_BYTES, MAX_PACKET_FILES + 1, || observer
            .remaining()
            .map(|_| ()))?
            == expected,
        "uploaded provider ZIP members differ"
    );
    for producer in &receipt.producers {
        let fresh = observer.json(&format!("actions/runs/{}", producer.run_id))?;
        validate_run(&fresh, spec, identity, producer.run_id, &producer.workflow)?;
        successful_run(&fresh)?;
    }
    let fresh_observer = observer.json(&format!("actions/runs/{}", identity.run_id))?;
    let workflow = identity
        .workflow_ref
        .split('@')
        .next()
        .context("missing observer workflow path")?
        .strip_prefix(&format!("{}/", spec.repository))
        .context("observer workflow repository changed")?;
    validate_run(&fresh_observer, spec, identity, identity.run_id, workflow)?;
    let fresh = observer.json(&format!("actions/artifacts/{artifact_id}"))?;
    validate_artifact(&fresh, identity, artifact_id, upload_digest)?;
    observer.remaining()?;
    let result = json!({"schema":"simit.original-retention.v1","qualified":false,"capture_complete":true,"artifact":fresh,"head":identity.head,"event":identity.event,"observer_run":identity.run_id,"observer_attempt":1,"producer_count":receipt.producers.len()});
    write_json(&retained.join("retention.json"), &result)?;
    Ok(result)
}

fn validate_artifact(
    artifact: &Value,
    identity: &Identity,
    id: u64,
    expected_digest: &str,
) -> Result<()> {
    ensure!(
        artifact["id"].as_u64() == Some(id)
            && artifact["expired"] == false
            && artifact["digest"] == format!("sha256:{expected_digest}")
            && artifact["workflow_run"]["id"].as_u64() == Some(identity.run_id)
            && artifact["workflow_run"]["head_sha"] == identity.head,
        "uploaded artifact identity or digest changed"
    );
    let created = chrono::DateTime::parse_from_rfc3339(
        artifact["created_at"]
            .as_str()
            .context("missing artifact creation")?,
    )?;
    let expires = chrono::DateTime::parse_from_rfc3339(
        artifact["expires_at"]
            .as_str()
            .context("missing artifact expiry")?,
    )?;
    ensure!(
        (expires - created).num_seconds() >= 2592000 && expires > chrono::Utc::now(),
        "uploaded artifact retention is insufficient"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_receipt() -> Receipt {
        let (spec, identity) = minimal_inputs();
        Receipt {
            schema: "simit.original-custody.v1".into(),
            qualified: false,
            capture_complete: false,
            spec,
            identity,
            producers: Vec::new(),
            error: None,
            members: BTreeMap::new(),
        }
    }

    #[test]
    fn aggregate_member_maps_cannot_poison_the_later_incomplete_receipt() {
        const LIMIT: u64 = 8192;
        let root = tempfile::tempdir().unwrap();
        let mut receipt = pending_receipt();
        save_receipt_with_limit(root.path(), &receipt, LIMIT).unwrap();
        fs::write(
            root.path().join("original-provider.zip"),
            b"unchanged original archive bytes",
        )
        .unwrap();
        let mut admitted = 0;
        let failure = loop {
            let producer = Producer {
                run_id: 12 + admitted,
                workflow: ".github/workflows/ci.yaml".into(),
                terminal: json!({"status":"completed","conclusion":"success"}),
                raw_logs_sha256: "a".repeat(64),
                log_members: [(
                    format!("logs/{admitted}/{}", "x".repeat(2000)),
                    "b".repeat(64),
                )]
                .into(),
            };
            match append_producer(root.path(), &mut receipt, producer, LIMIT) {
                Ok(()) => admitted += 1,
                Err(error) => break error,
            }
        };
        assert!(admitted > 0);
        assert_eq!(
            receipt.producers.len(),
            admitted as usize,
            "overflowing producer must not remain duplicated into the receipt"
        );
        receipt.error = Some(failure.to_string());
        receipt.members.insert(
            "original-provider.zip".into(),
            digest(b"unchanged original archive bytes"),
        );
        assert!(publish_final_receipt(root.path(), &mut receipt, LIMIT).is_err());
        let retained: Receipt =
            serde_json::from_slice(&fs::read(root.path().join("receipt.json")).unwrap()).unwrap();
        assert!(retained.error.is_some());
        assert!(
            retained
                .error
                .as_deref()
                .unwrap()
                .contains("aggregate producer receipt")
        );
        assert!(!retained.capture_complete);
        assert!(!retained.qualified);
        assert_eq!(retained.producers.len(), admitted as usize);
        assert_eq!(
            fs::read(root.path().join("original-provider.zip")).unwrap(),
            b"unchanged original archive bytes"
        );
    }

    #[test]
    fn final_receipt_growth_publishes_an_explicit_failure_instead_of_stale_pending_state() {
        const LIMIT: u64 = 8192;
        let root = tempfile::tempdir().unwrap();
        let mut receipt = pending_receipt();
        save_receipt_with_limit(root.path(), &receipt, LIMIT).unwrap();
        fs::write(
            root.path().join("original-response.body"),
            b"original interrupted provider response",
        )
        .unwrap();
        receipt.capture_complete = true;
        for index in 0..100 {
            receipt.members.insert(
                format!("requests/{index}/{}", "x".repeat(100)),
                "a".repeat(64),
            );
        }
        assert!(publish_final_receipt(root.path(), &mut receipt, LIMIT).is_err());
        let retained: Receipt =
            serde_json::from_slice(&fs::read(root.path().join("receipt.json")).unwrap()).unwrap();
        assert!(!retained.capture_complete);
        assert!(retained.error.is_some());
        assert!(!retained.qualified);
        assert_eq!(
            fs::read(root.path().join("original-response.body")).unwrap(),
            b"original interrupted provider response"
        );
    }

    #[cfg(unix)]
    #[test]
    fn inventory_rejects_names_that_cannot_be_portable_zip_members() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("bad\\name"), b"evidence").unwrap();
        assert!(inventory(root.path()).is_err());
    }

    struct MaximumProbe(u64, Duration);
    impl Transport for MaximumProbe {
        fn get(
            &mut self,
            _endpoint: &str,
            maximum: u64,
            timeout: Duration,
            _cancellation: &Cancellation,
        ) -> Result<Response> {
            self.0 = maximum;
            self.1 = timeout;
            Ok(Response {
                status: 200,
                body: Vec::new(),
                retry_after: None,
                transport_error: None,
            })
        }
    }

    #[test]
    fn retention_metadata_cannot_truncate_the_largest_admitted_packet_download() {
        let (spec, _) = minimal_inputs();
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("requests")).unwrap();
        let mut transport = MaximumProbe(0, Duration::ZERO);
        let mut clock = MonotonicClock::default();
        let cancellation = Cancellation::default();
        let mut observer = Observer {
            spec: &spec,
            root: root.path(),
            transport: &mut transport,
            clock: &mut clock,
            cancellation: &cancellation,
            requests: 0,
            bytes: 2 * JSON_BYTES,
            byte_budget: RETENTION_BYTES,
        };
        observer
            .get("actions/artifacts/1/zip", ARTIFACT_BYTES)
            .unwrap();
        assert_eq!(transport.0, ARTIFACT_BYTES);
        assert!(transport.0 > 2 * TOTAL_BYTES);
        // The wire download must fit full provider-body/copy custody plus every
        // bounded generated file (event/spec, three snapshots per workflow,
        // all request diagnostics), and the final receipt.
        let generated = (2 + 3 * 10) * JSON_BYTES + MAX_REQUESTS as u64 * 4096;
        assert!(transport.0 >= 2 * TOTAL_BYTES + generated + RECEIPT_BYTES);
        assert!(transport.1 > Duration::from_secs(90));
    }

    #[test]
    fn uploaded_zip64_can_cover_the_admitted_packet_inventory() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..=u16::MAX as usize {
            writer
                .start_file(
                    format!("requests/{index:06}.body"),
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(b"original bytes").unwrap();
        }
        let raw = writer.finish().unwrap().into_inner();
        let admitted = archive_members(&raw, 2 * TOTAL_BYTES, MAX_PACKET_FILES, || Ok(())).unwrap();
        assert_eq!(admitted.len(), u16::MAX as usize + 1);
        assert!(log_members(&raw, || Ok(())).is_err());
    }

    #[test]
    fn receipts_bind_validated_identity_without_copying_ignored_provider_payloads() {
        let run = json!({"id":1,"repository":{"id":1,"full_name":"owner/repo","ignored":"x".repeat(1024*1024)},"head_repository":{"id":1,"full_name":"owner/repo"},"head_sha":"a".repeat(40),"head_branch":"main","event":"push","run_attempt":1,"path":".github/workflows/ci.yaml","status":"completed","conclusion":"success","ignored":"x".repeat(1024*1024)});
        let (spec, identity) = minimal_inputs();
        let compact = compact_run(&run);
        validate_run(&compact, &spec, &identity, 1, ".github/workflows/ci.yaml").unwrap();
        assert!(serde_json::to_vec(&compact).unwrap().len() < 1024);
    }

    #[test]
    fn failed_receipt_serialization_cannot_block_a_subsequent_failure_receipt() {
        let root = tempfile::tempdir().unwrap();
        let (spec, identity) = minimal_inputs();
        let mut receipt = Receipt {
            schema: "simit.original-custody.v1".into(),
            qualified: false,
            capture_complete: false,
            spec,
            identity,
            producers: Vec::new(),
            error: None,
            members: BTreeMap::new(),
        };
        save_receipt(root.path(), &receipt).unwrap();
        receipt.error = Some("x".repeat(64 * 1024 * 1024));
        assert!(save_receipt(root.path(), &receipt).is_err());
        receipt.error = Some("explicit interrupted receipt serialization".into());
        save_receipt(root.path(), &receipt).unwrap();
        let retained: Receipt =
            serde_json::from_slice(&fs::read(root.path().join("receipt.json")).unwrap()).unwrap();
        assert_eq!(retained.error, receipt.error);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    fn minimal_inputs() -> (Spec, Identity) {
        let spec = Spec {
            schema: "simit.original-observer-spec.v1".into(),
            repository: "owner/repo".into(),
            workflows: [(".github/workflows/ci.yaml".into(), vec!["test".into()])].into(),
            timeout_seconds: 120,
            poll_seconds: 10,
        };
        let identity=Identity {repository_id:1,head:"a".repeat(40),event:"push".into(),run_id:2,run_attempt:1,workflow_ref:"owner/repo/.github/workflows/retain.yaml@refs/heads/main".into(),workflow_sha:"a".repeat(40),event_json:json!({"after":"a".repeat(40),"ref":"refs/heads/main","deleted":false,"repository":{"id":1,"full_name":"owner/repo"}}).to_string()};
        (spec, identity)
    }
}
