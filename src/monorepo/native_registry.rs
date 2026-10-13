//! Read-only exact-version discovery through the native public registry APIs.
use std::process::Command;

use anyhow::{Context, Result, ensure};
use semver::Version;
use serde_json::Value;

use super::releases::Package;

/// Core Metadata distribution names are ASCII and alphanumeric at both ends.
/// https://packaging.python.org/en/latest/specifications/name-normalization/
pub(crate) fn valid_python_package_name(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Preserve npm 11.4.2's legacy publication names, including leading hyphens.
/// https://github.com/npm/validate-npm-package-name/blob/v6.0.0/lib/index.js
pub(crate) fn valid_npm_package_name(name: &str) -> bool {
    let url_friendly = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
                    )
            })
    };
    if name.is_empty()
        || name.starts_with(['.', '_'])
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "node_modules" | "favicon.ico"
        )
    {
        return false;
    }
    if let Some(scoped) = name.strip_prefix('@') {
        scoped.split_once('/').is_some_and(|(scope, package)| {
            url_friendly(scope) && url_friendly(package) && !package.starts_with('.')
        })
    } else {
        url_friendly(name)
    }
}

/// npm tags share a namespace with versions and cannot be SemVer ranges.
/// Within our ASCII tag grammar, only v-prefixed versions/x-ranges and bare
/// x-ranges can be ranges; operators, whitespace and build metadata are excluded.
pub(crate) fn valid_npm_dist_tag(tag: &str) -> bool {
    if !tag.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        || !tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    {
        return false;
    }
    let candidate = tag.trim_start_matches('v');
    let (core, pre) = candidate
        .split_once('-')
        .map_or((candidate, None), |(core, pre)| (core, Some(pre)));
    let parts: Vec<_> = core.split('.').collect();
    if parts.len() > 3
        || pre.is_some_and(|pre| {
            parts.len() != 3 || pre.is_empty() || semver::Prerelease::new(pre).is_err()
        })
    {
        return true;
    }
    let mut wildcard = false;
    let is_range = parts.iter().all(|part| {
        if matches!(*part, "x" | "X") {
            wildcard = true;
            true
        } else {
            !wildcard
                && (*part == "0" || !part.starts_with('0'))
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && part
                    .parse::<u64>()
                    .is_ok_and(|number| number <= 9_007_199_254_740_991)
        }
    });
    if !is_range || (!wildcard && parts.len() == 3 && tag.len() - candidate.len() > 1) {
        return true;
    }
    if parts.len() < 3 || wildcard {
        // npm expands partial numeric ranges to an exclusive next version.
        // An increment beyond JavaScript's safe integer ceiling is not a range.
        if let Some(last) = parts
            .iter()
            .take_while(|part| !matches!(**part, "x" | "X"))
            .last()
        {
            return last
                .parse::<u64>()
                .is_ok_and(|number| number == 9_007_199_254_740_991);
        }
    }
    false
}

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
                        let is_type = |file: &Value, kind: &str, suffix: &str| {
                            file["packagetype"] == kind
                                && file["filename"]
                                    .as_str()
                                    .is_some_and(|name| name.ends_with(suffix))
                        };
                        files.len() == 2
                            && files
                                .iter()
                                .filter(|file| is_type(file, "bdist_wheel", ".whl"))
                                .count()
                                == 1
                            && files
                                .iter()
                                .filter(|file| is_type(file, "sdist", ".tar.gz"))
                                .count()
                                == 1
                            && files.iter().all(|file| {
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
    #[test]
    fn python_verification_requires_one_unyanked_wheel_and_sdist() {
        let complete = serde_json::json!({"info":{"name":"Py_Engine","version":"1.2.3"},"urls":[
            {"filename":"py_engine-1.2.3-py3-none-any.whl","packagetype":"bdist_wheel","yanked":false,"digests":{"sha256":"a".repeat(64)}},
            {"filename":"py_engine-1.2.3.tar.gz","packagetype":"sdist","yanked":false,"digests":{"sha256":"b".repeat(64)}}
        ]});
        assert!(Registry::Python.available(&complete, "py-engine", "1.2.3"));
        for index in 0..2 {
            let mut partial = complete.clone();
            partial["urls"].as_array_mut().unwrap().remove(index);
            assert!(!Registry::Python.available(&partial, "py-engine", "1.2.3"));
            for (field, invalid) in [
                ("yanked", serde_json::json!(true)),
                ("digests", serde_json::json!({"sha256":"bad"})),
                ("filename", serde_json::json!("unrelated.zip")),
                ("packagetype", serde_json::json!("bdist_egg")),
            ] {
                let mut bad = complete.clone();
                bad["urls"][index][field] = invalid;
                assert!(
                    !Registry::Python.available(&bad, "py-engine", "1.2.3"),
                    "{field}: {bad}"
                );
            }
        }
        let mut extra = complete.clone();
        extra["urls"]
            .as_array_mut()
            .unwrap()
            .push(complete["urls"][0].clone());
        assert!(!Registry::Python.available(&extra, "py-engine", "1.2.3"));
        let mut wheels = complete.clone();
        wheels["urls"][1] = complete["urls"][0].clone();
        assert!(!Registry::Python.available(&wheels, "py-engine", "1.2.3"));
    }

    #[test]
    fn npm_tags_exclude_semver_ranges_but_keep_named_channels() {
        for tag in [
            "v1",
            "v1.4",
            "vv1.4",
            "v1.2.3",
            "v1.2.3-beta.1",
            "v1.x",
            "v1.2.x-beta",
            "vx.x.x-beta",
            "x",
            "X",
            "X.X.X",
        ] {
            assert!(!valid_npm_dist_tag(tag), "{tag}");
        }
        for tag in [
            "latest",
            "next",
            "stable",
            "release-candidate",
            "vnext",
            "v1-beta",
            "v1.4-beta",
            "V1.4",
            "v01.4",
            "v1.2.3-",
            "v1.2.3-beta_1",
            "v1.2.3-beta.01",
            "xfoo",
            "vv1.2.3",
            "v9007199254740991",
            "v0.9007199254740991",
        ] {
            assert!(valid_npm_dist_tag(tag), "{tag}");
        }
    }
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
