use std::fmt::Write as _;

use crate::config::{HomebrewPlatformsConfig, ResolvedHomebrew};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Platform {
    DarwinArm,
    DarwinIntel,
    LinuxArm,
    LinuxIntel,
}

impl Platform {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "darwin_arm" => Ok(Self::DarwinArm),
            "darwin_intel" => Ok(Self::DarwinIntel),
            "linux_arm" => Ok(Self::LinuxArm),
            "linux_intel" => Ok(Self::LinuxIntel),
            other => Err(format!(
                "unknown platform {other}; expected darwin_arm, darwin_intel, linux_arm, or linux_intel"
            )),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::DarwinArm => "darwin_arm",
            Self::DarwinIntel => "darwin_intel",
            Self::LinuxArm => "linux_arm",
            Self::LinuxIntel => "linux_intel",
        }
    }

    pub fn arch(self) -> &'static str {
        match self {
            Self::DarwinArm | Self::LinuxArm => "aarch64",
            Self::DarwinIntel | Self::LinuxIntel => "x86_64",
        }
    }

    pub fn os(self) -> &'static str {
        match self {
            Self::DarwinArm | Self::DarwinIntel => "darwin",
            Self::LinuxArm | Self::LinuxIntel => "linux",
        }
    }
}

#[derive(Debug, Default)]
pub struct Sha256Set {
    pub darwin_arm: Option<String>,
    pub darwin_intel: Option<String>,
    pub linux_arm: Option<String>,
    pub linux_intel: Option<String>,
}

impl Sha256Set {
    pub fn all_no_check() -> Self {
        Self::default()
    }

    pub fn set(&mut self, platform: Platform, value: String) {
        *self.slot_mut(platform) = Some(value);
    }

    fn get(&self, platform: Platform) -> Option<&str> {
        match platform {
            Platform::DarwinArm => self.darwin_arm.as_deref(),
            Platform::DarwinIntel => self.darwin_intel.as_deref(),
            Platform::LinuxArm => self.linux_arm.as_deref(),
            Platform::LinuxIntel => self.linux_intel.as_deref(),
        }
    }

    fn slot_mut(&mut self, platform: Platform) -> &mut Option<String> {
        match platform {
            Platform::DarwinArm => &mut self.darwin_arm,
            Platform::DarwinIntel => &mut self.darwin_intel,
            Platform::LinuxArm => &mut self.linux_arm,
            Platform::LinuxIntel => &mut self.linux_intel,
        }
    }
}

pub fn render(resolved: &ResolvedHomebrew, version: &str, sha256s: &Sha256Set) -> String {
    let mut lines = vec![
        format!("class {} < Formula", class_name(&resolved.name)),
        format!("  desc {}", ruby_string(&resolved.description)),
        format!("  homepage {}", ruby_string(&resolved.homepage)),
        format!("  version {}", ruby_string(version)),
        format!("  license {}", ruby_string(&resolved.license)),
        String::new(),
    ];

    let platform_lines = platform_lines(resolved, version, sha256s);
    lines.extend(indent_lines("  ", &platform_lines));
    lines.push(String::new());
    lines.extend(indent_lines(
        "  ",
        &join_blocks(vec![install_lines(&resolved.binaries)]),
    ));
    lines.push("end".to_string());

    lines.join("\n")
}

pub fn class_name(name: &str) -> String {
    name.split('-').map(upper_first).collect()
}

pub fn archive_file_name(resolved: &ResolvedHomebrew, version: &str, platform: Platform) -> String {
    resolved
        .archive_pattern
        .replace("{name}", &resolved.name)
        .replace("{version}", version)
        .replace("{arch}", platform.arch())
        .replace("{os}", platform.os())
}

pub fn archive_url(resolved: &ResolvedHomebrew, version: &str, platform: Platform) -> String {
    format!(
        "https://codeberg.org/{}/releases/download/{}/{}",
        resolved.download_repo,
        version,
        archive_file_name(resolved, version, platform)
    )
}

pub fn enabled_platforms(platforms: &HomebrewPlatformsConfig) -> Vec<Platform> {
    [
        (Platform::DarwinArm, platforms.darwin_arm),
        (Platform::DarwinIntel, platforms.darwin_intel),
        (Platform::LinuxArm, platforms.linux_arm),
        (Platform::LinuxIntel, platforms.linux_intel),
    ]
    .into_iter()
    .filter_map(|(platform, enabled)| enabled.then_some(platform))
    .collect()
}

fn platform_lines(resolved: &ResolvedHomebrew, version: &str, sha256s: &Sha256Set) -> Vec<String> {
    join_blocks(vec![
        os_block_lines(
            resolved,
            version,
            sha256s,
            "macos",
            Platform::DarwinArm,
            Platform::DarwinIntel,
        ),
        os_block_lines(
            resolved,
            version,
            sha256s,
            "linux",
            Platform::LinuxArm,
            Platform::LinuxIntel,
        ),
    ])
}

fn os_block_lines(
    resolved: &ResolvedHomebrew,
    version: &str,
    sha256s: &Sha256Set,
    os: &str,
    arm: Platform,
    intel: Platform,
) -> Vec<String> {
    let arch_lines = join_blocks(vec![
        arch_block_lines(resolved, version, sha256s, arm, "arm"),
        arch_block_lines(resolved, version, sha256s, intel, "intel"),
    ]);
    if arch_lines.is_empty() {
        Vec::new()
    } else {
        let mut lines = vec![format!("on_{os} do")];
        lines.extend(indent_lines("  ", &arch_lines));
        lines.push("end".to_string());
        lines
    }
}

fn arch_block_lines(
    resolved: &ResolvedHomebrew,
    version: &str,
    sha256s: &Sha256Set,
    platform: Platform,
    arch: &str,
) -> Vec<String> {
    if !enabled_platforms(&resolved.platforms).contains(&platform) {
        return Vec::new();
    }

    vec![
        format!("on_{arch} do"),
        format!(
            "  url {}",
            ruby_string(&archive_url(resolved, version, platform))
        ),
        format!("  {}", render_sha256(sha256s.get(platform))),
        "end".to_string(),
    ]
}

fn render_sha256(sha256: Option<&str>) -> String {
    match sha256 {
        Some(value) => format!("sha256 {}", ruby_string(value)),
        None => "sha256 :no_check".to_string(),
    }
}

fn install_lines(binaries: &[String]) -> Vec<String> {
    let mut lines = vec!["def install".to_string()];
    lines.extend(
        binaries
            .iter()
            .map(|binary| format!("  bin.install {}", ruby_string(binary))),
    );
    lines.push("end".to_string());
    lines
}

fn indent_lines(prefix: &str, lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect()
}

fn join_blocks(blocks: Vec<Vec<String>>) -> Vec<String> {
    let mut joined = Vec::new();
    for block in blocks.into_iter().filter(|block| !block.is_empty()) {
        if !joined.is_empty() {
            joined.push(String::new());
        }
        joined.extend(block);
    }
    joined
}

fn upper_first(part: &str) -> String {
    let mut chars = part.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_uppercase().chain(chars).collect()
}

fn ruby_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => {
                write!(out, "\\u{:04x}", u32::from(ch)).expect("write to String cannot fail");
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::class_name;

    #[test]
    fn class_name_matches_homebrew_formula_expectation() {
        assert_eq!(class_name("modde"), "Modde");
        assert_eq!(class_name("my-app"), "MyApp");
        assert_eq!(class_name("rs-harbor"), "RsHarbor");
    }
}
