use crate::config::{ResolvedScoop, ScoopArchSet};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Architecture {
    X64,
    Arm64,
}

impl Architecture {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "x64" | "64bit" => Ok(Self::X64),
            "arm64" => Ok(Self::Arm64),
            other => Err(format!(
                "unknown architecture {other}; expected x64 or arm64"
            )),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::Arm64 => "arm64",
        }
    }

    pub fn manifest_key(self) -> &'static str {
        match self {
            Self::X64 => "64bit",
            Self::Arm64 => "arm64",
        }
    }

    pub fn arch(self) -> &'static str {
        match self {
            Self::X64 => "x86_64",
            Self::Arm64 => "aarch64",
        }
    }
}

#[derive(Debug, Default)]
pub struct ScoopChecksums {
    pub x64: Option<String>,
    pub arm64: Option<String>,
}

impl ScoopChecksums {
    pub fn all_placeholder() -> Self {
        Self {
            x64: Some(placeholder_hash()),
            arm64: Some(placeholder_hash()),
        }
    }

    pub fn set(&mut self, architecture: Architecture, value: String) {
        *self.slot_mut(architecture) = Some(value);
    }

    fn get(&self, architecture: Architecture) -> Option<&str> {
        match architecture {
            Architecture::X64 => self.x64.as_deref(),
            Architecture::Arm64 => self.arm64.as_deref(),
        }
    }

    fn slot_mut(&mut self, architecture: Architecture) -> &mut Option<String> {
        match architecture {
            Architecture::X64 => &mut self.x64,
            Architecture::Arm64 => &mut self.arm64,
        }
    }
}

pub fn render(resolved: &ResolvedScoop, version: &str, checksums: &ScoopChecksums) -> String {
    let architectures = enabled_architectures(&resolved.architectures);
    let mut lines = vec![
        "{".to_owned(),
        format!("  \"version\": {},", json_string(version)),
        format!("  \"description\": {},", json_string(&resolved.description)),
        format!("  \"homepage\": {},", json_string(&resolved.homepage)),
        format!("  \"license\": {},", json_string(&resolved.license)),
        "  \"architecture\": {".to_owned(),
    ];

    for (index, architecture) in architectures.iter().copied().enumerate() {
        let arch_comma = if index + 1 == architectures.len() {
            ""
        } else {
            ","
        };
        lines.push(format!("    \"{}\": {{", architecture.manifest_key()));
        lines.push(format!(
            "      \"url\": {},",
            json_string(&archive_url(resolved, version, architecture))
        ));
        lines.push(format!(
            "      \"hash\": {},",
            json_string(checksums.get(architecture).unwrap_or(PLACEHOLDER_HASH))
        ));
        lines.push(format!("      \"bin\": {}", render_bin(&resolved.binaries)));
        lines.push(format!("    }}{arch_comma}"));
    }

    lines.push("  }".to_owned());
    lines.push("}".to_owned());
    lines.push(String::new());
    lines.join("\n")
}

pub fn archive_file_name(
    resolved: &ResolvedScoop,
    version: &str,
    architecture: Architecture,
) -> String {
    resolved
        .archive_pattern
        .replace("{name}", &resolved.name)
        .replace("{version}", version)
        .replace("{arch}", architecture.arch())
}

pub fn archive_url(resolved: &ResolvedScoop, version: &str, architecture: Architecture) -> String {
    format!(
        "https://codeberg.org/{}/releases/download/{}/{}",
        resolved.download_repo,
        version,
        archive_file_name(resolved, version, architecture)
    )
}

pub fn enabled_architectures(architectures: &ScoopArchSet) -> Vec<Architecture> {
    [
        (Architecture::X64, architectures.x64),
        (Architecture::Arm64, architectures.arm64),
    ]
    .into_iter()
    .filter_map(|(architecture, enabled)| enabled.then_some(architecture))
    .collect()
}

fn render_bin(binaries: &[String]) -> String {
    if let [binary] = binaries {
        json_string(binary)
    } else {
        let values = binaries
            .iter()
            .map(|binary| json_string(binary))
            .collect::<Vec<_>>();
        format!("[{}]", values.join(", "))
    }
}

fn placeholder_hash() -> String {
    PLACEHOLDER_HASH.to_owned()
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing string to JSON cannot fail")
}

const PLACEHOLDER_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
