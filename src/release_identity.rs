//! Keep semantic package versions distinct from their Git release tags.
use std::fmt;

use anyhow::{Result, bail};
use semver::Version;
use serde::Deserialize;

/// Supported release-tag conventions. The default preserves unprefixed tags.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
pub enum TagPrefix {
    #[default]
    #[serde(rename = "")]
    None,
    #[serde(rename = "v")]
    V,
}

impl TagPrefix {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::V => "v",
        }
    }

    pub fn tag(self, version: Version) -> ReleaseTag {
        ReleaseTag {
            version,
            prefix: self,
            namespace: None,
        }
    }

    pub fn parse(self, tag: &str) -> Result<ReleaseTag> {
        let Some(raw) = tag.strip_prefix(self.as_str()) else {
            bail!(
                "release tag {tag:?} does not use prefix {:?}",
                self.as_str()
            );
        };
        let version = Version::parse(raw)?;
        let identity = self.tag(version);
        if identity.to_string() != tag {
            bail!("release tag {tag:?} is not canonical");
        }
        Ok(identity)
    }
}

/// A canonical tag with its unprefixed semantic version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseTag {
    pub version: Version,
    prefix: TagPrefix,
    namespace: Option<String>,
}

impl fmt::Display for ReleaseTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(namespace) = &self.namespace {
            write!(f, "{namespace}/")?;
        }
        write!(f, "{}{}", self.prefix.as_str(), self.version)
    }
}

impl ReleaseTag {
    /// Package names form independent release namespaces within one repository.
    pub fn for_package(package: &str, version: Version) -> Result<Self> {
        if package.is_empty()
            || !package
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("invalid release package namespace {package:?}");
        }
        Ok(Self {
            version,
            prefix: TagPrefix::V,
            namespace: Some(package.to_owned()),
        })
    }

    pub fn parse_for_package(package: &str, tag: &str) -> Result<Self> {
        let raw = tag
            .strip_prefix(&format!("{package}/v"))
            .ok_or_else(|| anyhow::anyhow!("release tag {tag:?} does not belong to {package}"))?;
        let identity = Self::for_package(package, Version::parse(raw)?)?;
        if identity.to_string() != tag {
            bail!("release tag {tag:?} is not canonical");
        }
        Ok(identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_round_trip_without_changing_versions() {
        for (prefix, tag) in [(TagPrefix::None, "0.2.0"), (TagPrefix::V, "v0.2.0-rc.1")] {
            let identity = prefix.parse(tag).unwrap();
            assert_eq!(identity.to_string(), tag);
            assert!(!identity.version.to_string().starts_with('v'));
        }
        assert!(TagPrefix::None.parse("v0.2.0").is_err());
        assert!(TagPrefix::V.parse("0.2.0").is_err());
        assert!(TagPrefix::V.parse("vv0.2.0").is_err());
    }

    #[test]
    fn package_tags_have_disjoint_release_namespaces() {
        let tag = ReleaseTag::parse_for_package("harbor-cache", "harbor-cache/v0.1.1").unwrap();
        assert_eq!(tag.version.to_string(), "0.1.1");
        assert_eq!(tag.to_string(), "harbor-cache/v0.1.1");
        assert!(ReleaseTag::parse_for_package("harbor-sdk", &tag.to_string()).is_err());
        assert!(ReleaseTag::for_package("../outside", Version::new(1, 0, 0)).is_err());
    }
}
