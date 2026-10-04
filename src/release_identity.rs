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
}

impl fmt::Display for ReleaseTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.prefix.as_str(), self.version)
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
}
