//! Bind an original run to the source event's branch or exact PR target.
use super::{Identity, Spec};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

fn reference(value: &Value) -> Result<&str> {
    let reference = value.as_str().context("missing source reference")?;
    ensure!(
        !reference.is_empty()
            && reference.len() <= 2048
            && !reference.chars().any(char::is_control),
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
    if let Some((_, suffix)) = run["path"]
        .as_str()
        .context("missing producer workflow path")?
        .split_once('@')
    {
        // Ref-qualified paths must describe the same event target as the run.
        // Normalization must not hide a contradictory executed-workflow ref.
        let branch = reference(&run["head_branch"])?;
        let mut allowed = vec![
            branch.to_owned(),
            format!("refs/heads/{branch}"),
            identity.head.clone(),
        ];
        if identity.event == "pull_request" {
            let number = event["pull_request"]["number"]
                .as_u64()
                .context("missing source PR number")?;
            allowed.extend([
                format!("refs/pull/{number}/head"),
                format!("refs/pull/{number}/merge"),
            ]);
        }
        ensure!(
            allowed.iter().any(|reference| reference == suffix),
            "producer workflow reference target changed"
        );
    }
    Ok(())
}
