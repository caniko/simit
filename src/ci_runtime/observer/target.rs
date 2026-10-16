//! Bind an original run to the source event's branch or exact PR target.
use super::{Identity, Spec};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

fn reference(value: &Value) -> Result<&str> {
    let reference = value.as_str().context("missing source reference")?;
    ensure!(
        !reference.is_empty()
            && reference.len() <= 2048
            && !reference.bytes().any(|byte| byte.is_ascii_control()),
        "invalid source reference"
    );
    Ok(reference)
}

fn repository_id(value: &Value) -> Result<u64> {
    let id = value.as_u64().context("missing source repository id")?;
    ensure!(id > 0, "invalid source repository id");
    Ok(id)
}

pub(super) fn validate(run: &Value, spec: &Spec, identity: &Identity) -> Result<()> {
    let event: Value = serde_json::from_str(&identity.event_json)?;
    match identity.event.as_str() {
        "push" => {
            let branch = reference(&event["ref"])?
                .strip_prefix("refs/heads/")
                .filter(|branch| !branch.is_empty())
                .context("missing source branch")?;
            ensure!(
                run["head_branch"] == branch
                    && run["head_repository"]["full_name"] == spec.repository
                    && run["head_repository"]["id"].as_u64() == Some(identity.repository_id),
                "producer branch identity changed"
            );
        }
        "pull_request" => {
            let expected = &event["pull_request"];
            let number = expected["number"]
                .as_u64()
                .context("missing source PR number")?;
            ensure!(number > 0, "invalid source PR number");
            let head_ref = reference(&expected["head"]["ref"])?;
            let base_ref = reference(&expected["base"]["ref"])?;
            let head_id = repository_id(&expected["head"]["repo"]["id"])?;
            let base_id = repository_id(&expected["base"]["repo"]["id"])?;
            let base_sha = expected["base"]["sha"]
                .as_str()
                .context("missing source PR base")?;
            ensure!(super::oid(base_sha), "invalid source PR base");
            ensure!(
                expected["head"]["sha"] == identity.head
                    && expected["base"]["repo"]["full_name"] == spec.repository
                    && run["head_branch"] == head_ref
                    && run["head_repository"]["id"] == head_id
                    && run["repository"]["id"] == base_id,
                "producer PR repository or branch identity changed"
            );
            let associations = run["pull_requests"]
                .as_array()
                .context("missing producer PR association")?;
            ensure!(associations.len() == 1, "ambiguous producer PR association");
            let actual = &associations[0];
            ensure!(
                actual["number"] == number
                    && actual["head"]["ref"] == head_ref
                    && actual["head"]["sha"] == identity.head
                    && actual["head"]["repo"]["id"] == head_id
                    && actual["base"]["ref"] == base_ref
                    && actual["base"]["sha"] == base_sha
                    && actual["base"]["repo"]["id"] == base_id,
                "producer PR target identity changed"
            );
        }
        _ => bail!("unsupported observer event"),
    }
    Ok(())
}

/// Repository-wide SHA discovery can include other branches and PRs. Skip only
/// identifiable other targets; selected originals still undergo full validation
/// here and on every later read, including ambiguous associations and drift.
pub(super) fn matches(run: &Value, identity: &Identity) -> Result<bool> {
    let event: Value = serde_json::from_str(&identity.event_json)?;
    match identity.event.as_str() {
        "push" => {
            let expected = reference(&event["ref"])?
                .strip_prefix("refs/heads/")
                .context("missing source branch")?;
            Ok(reference(&run["head_branch"])? == expected)
        }
        "pull_request" => {
            let expected = event["pull_request"]["number"]
                .as_u64()
                .filter(|number| *number > 0)
                .context("missing source PR number")?;
            let associations = run["pull_requests"]
                .as_array()
                .context("missing producer PR association")?;
            ensure!(!associations.is_empty(), "missing producer PR association");
            let mut selected = false;
            for association in associations {
                let number = association["number"]
                    .as_u64()
                    .filter(|number| *number > 0)
                    .context("invalid producer PR number")?;
                selected |= number == expected;
            }
            Ok(selected)
        }
        _ => bail!("unsupported observer event"),
    }
}

/// Git ref syntax with one-level names admitted. A provider workflow ref may
/// differ from the event target; branch/PR identity is bound by run fields above.
pub(super) fn workflow_reference(reference: &str) -> bool {
    !reference.is_empty()
        && reference.len() <= 2048
        && reference != "@"
        && !reference.contains("..")
        && !reference.contains("@{")
        && !reference
            .bytes()
            .any(|byte| byte <= b' ' || byte == 0x7f || b"~^:?*[\\".contains(&byte))
        && !reference.ends_with('.')
        && reference.split('/').all(|component| {
            !component.is_empty() && !component.starts_with('.') && !component.ends_with(".lock")
        })
}
