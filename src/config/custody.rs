//! Enrolled workflow identities and consumer-owned original-producer policy.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Protocol::is_legacy")]
    pub protocol: Protocol,
    pub repository: String,
    pub producer_attempt: u64,
    pub status_context: String,
    pub producers: BTreeMap<String, Producer>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum Protocol {
    #[default]
    #[serde(rename = "strict-v1")]
    StrictV1,
    #[serde(rename = "sealed-v2")]
    SealedV2,
}

impl Protocol {
    fn is_legacy(&self) -> bool {
        *self == Self::StrictV1
    }
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
            || self.status_context.len() + "/pull_request/".len() + 64 > 100
            || self.status_context.chars().any(char::is_control)
        {
            bail!(
                "[ci.custody].status_context must be a nonempty single-line prefix of at most 22 bytes for the composed 100-byte context"
            );
        }
        if self.producers.is_empty() || self.producers.len() > 10 {
            bail!("[ci.custody].producers must enroll between 1 and 10 workflows");
        }
        let inventory: usize = self
            .producers
            .values()
            .map(|producer| producer.jobs.len() + producer.artifacts.len())
            .sum();
        if inventory > 100 {
            bail!("[ci.custody] can enroll at most 100 jobs and artifacts in total");
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
