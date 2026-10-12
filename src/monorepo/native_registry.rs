//! Read-only exact-version discovery through the native public registry APIs.
use std::process::Command;

use anyhow::{Context, Result, ensure};
use semver::Version;
use serde_json::Value;

use super::releases::Package;

#[derive(Clone, Copy)]
pub(crate) enum Registry {
    Python,
    Npm,
}

impl Registry {
    pub(crate) fn for_manifest(manifest: &str) -> Self {
        if std::path::Path::new(manifest)
            .file_name()
            .is_some_and(|name| name == "pyproject.toml")
        {
            Self::Python
        } else {
            Self::Npm
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Npm => "npm",
        }
    }

    pub(crate) fn package_key(self, name: &str) -> String {
        match self {
            // https://packaging.python.org/en/latest/specifications/name-normalization/
            Self::Python => name
                .split(['-', '_', '.'])
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("-")
                .to_ascii_lowercase(),
            Self::Npm => name.to_owned(),
        }
    }

    pub(crate) fn version(self, version: &Version) -> Result<String> {
        ensure!(
            version.build.is_empty(),
            "public native releases cannot have SemVer build metadata"
        );
        if matches!(self, Self::Npm) {
            return Ok(version.to_string());
        }
        let base = format!("{}.{}.{}", version.major, version.minor, version.patch);
        if version.pre.is_empty() {
            return Ok(base);
        }
        let mut parts = version.pre.as_str().split('.');
        let label = parts.next().unwrap_or_default();
        let number = parts.next().unwrap_or("0");
        ensure!(
            parts.next().is_none() && number.bytes().all(|byte| byte.is_ascii_digit()),
            "Python publication requires alpha, beta, rc or dev prereleases with a numeric suffix"
        );
        let suffix = match label {
            "alpha" => "a",
            "beta" => "b",
            "rc" => "rc",
            "dev" => ".dev",
            _ => anyhow::bail!("Python publication requires alpha, beta, rc or dev prereleases"),
        };
        Ok(format!("{base}{suffix}{number}"))
    }

    fn url(self, name: &str, version: &str) -> Result<url::Url> {
        let mut url = url::Url::parse(match self {
            Self::Python => "https://pypi.org/pypi/",
            Self::Npm => "https://registry.npmjs.org/",
        })?;
        let mut parts = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("registry URL cannot be a base"))?;
        parts.pop_if_empty().push(name);
        if matches!(self, Self::Python) {
            parts.push(version).push("json");
        }
        drop(parts);
        Ok(url)
    }

    fn available(self, body: &Value, name: &str, version: &str) -> bool {
        match self {
            // The release-specific endpoint includes only this release's files.
            // https://docs.pypi.org/api/json/#get-a-release
            Self::Python => {
                body["info"]["name"]
                    .as_str()
                    .is_some_and(|published| self.package_key(published) == self.package_key(name))
                    && body["info"]["version"] == version
                    && body["urls"].as_array().is_some_and(|files| {
                        files.iter().any(|file| {
                            file["yanked"] == false
                                && file["digests"]["sha256"].as_str().is_some_and(|digest| {
                                    digest.len() == 64
                                        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                                })
                        })
                    })
            }
            // Use the abbreviated packument and its exact version record.
            // https://github.com/npm/registry/blob/main/docs/responses/package-metadata.md
            Self::Npm => {
                let release = &body["versions"][version];
                body["name"] == name
                    && release["name"] == name
                    && release["version"] == version
                    && release["dist"]["integrity"]
                        .as_str()
                        .is_some_and(|integrity| !integrity.is_empty())
            }
        }
    }
}

pub(crate) fn available(package: &Package, version: &Version) -> Result<bool> {
    let registry = Registry::for_manifest(&package.config.manifest);
    let version = registry.version(version)?;
    let url = registry.url(&package.name, &version)?;
    let output = Command::new("curl")
        .args([
            "--disable",
            "--silent",
            "--show-error",
            "--max-time",
            "10",
            "--max-filesize",
            "4194304",
            "--header",
            "Accept: application/vnd.npm.install-v1+json, application/json",
            "--user-agent",
            "simit release verify",
            "--write-out",
            "\n%{http_code}",
            url.as_str(),
        ])
        .output()
        .context("read native registry metadata with curl")?;
    ensure!(
        output.status.success(),
        "native registry request failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let text = std::str::from_utf8(&output.stdout)?;
    let (body, status) = text
        .rsplit_once('\n')
        .context("native registry response has no HTTP status")?;
    if status == "404" {
        return Ok(false);
    }
    ensure!(status == "200", "native registry returned HTTP {status}");
    ensure!(
        body.len() <= 4 * 1024 * 1024,
        "native registry metadata exceeds 4 MiB"
    );
    Ok(registry.available(&serde_json::from_str(body)?, &package.name, &version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_semver_prereleases_use_normalized_registry_versions() {
        let registry = Registry::Python;
        for (source, normalized) in [
            ("1.2.3", "1.2.3"),
            ("1.2.3-rc.4", "1.2.3rc4"),
            ("1.2.3-alpha", "1.2.3a0"),
            ("1.2.3-dev.5", "1.2.3.dev5"),
        ] {
            let version = registry.version(&Version::parse(source).unwrap()).unwrap();
            assert_eq!(version, normalized);
            assert_eq!(
                registry.url("engine", &version).unwrap().path(),
                format!("/pypi/engine/{normalized}/json")
            );
        }
        for version in [
            "1.2.3+build",
            "1.2.3-preview.1",
            "1.2.3-rc.one",
            "1.2.3-rc.1.2",
        ] {
            assert!(registry.version(&Version::parse(version).unwrap()).is_err());
        }
    }
}
