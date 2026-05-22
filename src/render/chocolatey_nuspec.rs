use std::fmt::Write as _;

use crate::config::ResolvedChocolatey;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Architecture {
    X64,
    X86,
}

impl Architecture {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "x64" => Ok(Self::X64),
            "x86" => Ok(Self::X86),
            other => Err(format!("unknown architecture {other}; expected x64 or x86")),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::X86 => "x86",
        }
    }

    pub fn archive_arch(self) -> &'static str {
        match self {
            Self::X64 => "x86_64",
            Self::X86 => "i686",
        }
    }
}

#[derive(Debug, Default)]
pub struct Sha256Set {
    pub x64: Option<String>,
    pub x86: Option<String>,
}

impl Sha256Set {
    pub fn all_no_check() -> Self {
        Self::default()
    }

    pub fn set(&mut self, arch: Architecture, value: String) {
        match arch {
            Architecture::X64 => self.x64 = Some(value),
            Architecture::X86 => self.x86 = Some(value),
        }
    }

    pub fn get(&self, arch: Architecture) -> Option<&str> {
        match arch {
            Architecture::X64 => self.x64.as_deref(),
            Architecture::X86 => self.x86.as_deref(),
        }
    }
}

#[derive(Debug)]
pub struct ChocolateyRenderOptions<'a> {
    pub resolved: &'a ResolvedChocolatey,
    pub version: &'a str,
    pub include_x86: bool,
}

#[derive(Debug)]
pub struct RenderedPackage {
    pub nuspec_name: String,
    pub nuspec: String,
    pub install_script: String,
    pub uninstall_script: String,
}

pub fn render_package(
    opts: &ChocolateyRenderOptions<'_>,
    checksums: &Sha256Set,
) -> RenderedPackage {
    RenderedPackage {
        nuspec_name: format!("{}.nuspec", opts.resolved.id),
        nuspec: render_nuspec(opts),
        install_script: render_install_script(opts, checksums),
        uninstall_script: render_uninstall_script(opts),
    }
}

pub fn render_nuspec(opts: &ChocolateyRenderOptions<'_>) -> String {
    let resolved = opts.resolved;
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<package xmlns=\"http://schemas.microsoft.com/packaging/2015/06/nuspec.xsd\">\n");
    out.push_str("  <metadata>\n");
    field(&mut out, "id", &resolved.id);
    field(&mut out, "version", opts.version);
    field(&mut out, "title", &resolved.title);
    field(
        &mut out,
        "authors",
        resolved.authors.as_deref().unwrap_or("UNKNOWN"),
    );
    field(&mut out, "description", &resolved.description);
    field(&mut out, "projectUrl", &resolved.project_url);
    if let Some(license_url) = &resolved.license_url {
        field(&mut out, "licenseUrl", license_url);
    }
    if let Some(tags) = &resolved.tags {
        field(&mut out, "tags", tags);
    } else {
        field(&mut out, "tags", &resolved.name);
    }
    if let Some(release_notes_url) = &resolved.release_notes_url {
        field(&mut out, "releaseNotes", release_notes_url);
    }
    out.push_str("  </metadata>\n");
    out.push_str("  <files>\n");
    out.push_str("    <file src=\"tools\\chocolateyInstall.ps1\" target=\"tools\" />\n");
    out.push_str("    <file src=\"tools\\chocolateyUninstall.ps1\" target=\"tools\" />\n");
    out.push_str("  </files>\n");
    out.push_str("</package>\n");
    out
}

pub fn render_install_script(opts: &ChocolateyRenderOptions<'_>, checksums: &Sha256Set) -> String {
    let resolved = opts.resolved;
    let x64_url = archive_url(resolved, opts.version, Architecture::X64);
    let x86_url = opts
        .include_x86
        .then(|| archive_url(resolved, opts.version, Architecture::X86));

    let mut out = String::new();
    out.push_str("$ErrorActionPreference = 'Stop'\n\n");
    line_var(&mut out, "packageName", &resolved.id);
    out.push_str("$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition\n");
    line_var(&mut out, "url64", &x64_url);
    if let Some(checksum) = checksums.get(Architecture::X64) {
        line_var(&mut out, "checksum64", checksum);
    }
    if let Some(x86_url) = x86_url {
        line_var(&mut out, "url", &x86_url);
        if let Some(checksum) = checksums.get(Architecture::X86) {
            line_var(&mut out, "checksum", checksum);
        }
    }
    out.push('\n');
    out.push_str("if (-not [Environment]::Is64BitOperatingSystem) {\n");
    if opts.include_x86 {
        out.push_str("  Install-ChocolateyZipPackage `\n");
        out.push_str("    -PackageName $packageName `\n");
        out.push_str("    -Url $url `\n");
        out.push_str("    -UnzipLocation $toolsDir");
        if checksums.get(Architecture::X86).is_some() {
            out.push_str(" `\n    -Checksum $checksum `\n    -ChecksumType 'sha256'");
        }
        out.push('\n');
        out.push_str("  return\n");
    } else {
        out.push_str("  throw \"$packageName requires a 64-bit Windows install.\"\n");
    }
    out.push_str("}\n\n");
    out.push_str("Install-ChocolateyZipPackage `\n");
    out.push_str("  -PackageName $packageName `\n");
    out.push_str("  -Url $url64 `\n");
    out.push_str("  -Url64bit $url64 `\n");
    out.push_str("  -UnzipLocation $toolsDir");
    if checksums.get(Architecture::X64).is_some() {
        out.push_str(" `\n  -Checksum64 $checksum64 `\n  -ChecksumType64 'sha256'");
    }
    out.push('\n');
    out
}

pub fn render_uninstall_script(_opts: &ChocolateyRenderOptions<'_>) -> String {
    "# No uninstall action is required for the default zip package layout.\n".to_owned()
}

pub fn archive_file_name(
    resolved: &ResolvedChocolatey,
    version: &str,
    arch: Architecture,
) -> String {
    resolved
        .archive_pattern
        .replace("{name}", &resolved.name)
        .replace("{version}", version)
        .replace("{arch}", arch.archive_arch())
}

pub fn archive_url(resolved: &ResolvedChocolatey, version: &str, arch: Architecture) -> String {
    format!(
        "https://codeberg.org/{}/releases/download/{}/{}",
        resolved.download_repo,
        version,
        archive_file_name(resolved, version, arch)
    )
}

fn field(out: &mut String, name: &str, value: &str) {
    writeln!(out, "    <{name}>{}</{name}>", xml_escape(value))
        .expect("write to String cannot fail");
}

fn line_var(out: &mut String, name: &str, value: &str) {
    writeln!(out, "${name} = '{}'", ps_single_quoted(value)).expect("write to String cannot fail");
}

fn xml_escape(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            ch => out.push(ch),
        }
    }
    out
}

fn ps_single_quoted(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::xml_escape;

    #[test]
    fn xml_escape_handles_special_chars() {
        assert_eq!(
            xml_escape("a & <b> \"c\" 'd'"),
            "a &amp; &lt;b&gt; &quot;c&quot; &apos;d&apos;"
        );
    }
}
