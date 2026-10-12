//! Enrolled workflow identities and consumer-owned original-producer policy.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub repository: String,
    pub producer_attempt: u64,
    pub status_context: String,
    pub producers: BTreeMap<String, Producer>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Producer {
    pub workflow: String,
    pub jobs: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<String>,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        let repo: Vec<_> = self.repository.split('/').collect();
        if repo.len() != 2
            || repo.iter().any(|part| {
                part.is_empty()
                    || !part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            })
        {
            bail!("[ci.custody].repository must be an owner/repository on github.com");
        }
        if self.producer_attempt != 1 {
            bail!(
                "[ci.custody].producer_attempt must enroll original attempt 1 for branch/PR producers"
            );
        }
        if self.status_context.is_empty()
            || self.status_context.len() > 100
            || self.status_context.chars().any(char::is_control)
        {
            bail!("[ci.custody].status_context must be a nonempty single-line context");
        }
        if self.producers.is_empty() || self.producers.len() > 10 {
            bail!("[ci.custody].producers must enroll between 1 and 10 workflows");
        }
        let mut paths = std::collections::BTreeSet::new();
        for (name, producer) in &self.producers {
            if producer.workflow == ".github/workflows/producer-custody.yaml"
                || name == "Retain completed producer custody"
            {
                bail!("[ci.custody] cannot enroll its own completion observer");
            }
            let basename = producer.workflow.strip_prefix(".github/workflows/");
            if name.is_empty()
                || name.chars().any(char::is_control)
                || basename.is_none_or(|path| {
                    path.contains('/')
                        || !(path.ends_with(".yaml") || path.ends_with(".yml"))
                        || !path
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                })
                || !paths.insert(&producer.workflow)
            {
                bail!("[ci.custody].producers requires unique workflow paths and nonempty names");
            }
            if producer.jobs.is_empty() {
                bail!("[ci.custody].producers must enroll nonempty required job inventories");
            }
            for values in [&producer.jobs, &producer.artifacts] {
                let unique = values.iter().collect::<std::collections::BTreeSet<_>>();
                if unique.len() != values.len()
                    || values
                        .iter()
                        .any(|value| value.is_empty() || value.chars().any(char::is_control))
                {
                    bail!(
                        "[ci.custody].producers inventories must be nonempty identifiers without duplicates"
                    );
                }
            }
        }
        Ok(())
    }
}
