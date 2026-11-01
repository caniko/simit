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
    if let Some(summary) = &resolved.summary {
        field(&mut out, "summary", summary);
    }
    field(&mut out, "projectUrl", &resolved.project_url);
    if let Some(license_url) = &resolved.license_url {
        field(&mut out, "licenseUrl", license_url);
    }
    if let Some(icon_url) = &resolved.icon_url {
        field(&mut out, "iconUrl", icon_url);
    }
    if let Some(package_source_url) = &resolved.package_source_url {
        field(&mut out, "packageSourceUrl", package_source_url);
    }
    if let Some(docs_url) = &resolved.docs_url {
        field(&mut out, "docsUrl", docs_url);
    }
    if let Some(bug_tracker_url) = &resolved.bug_tracker_url {
        field(&mut out, "bugTrackerUrl", bug_tracker_url);
    }
    if let Some(project_source_url) = &resolved.project_source_url {
        field(&mut out, "projectSourceUrl", project_source_url);
    }
    if let Some(tags) = &resolved.tags {
        field(&mut out, "tags", tags);
    } else {
        field(&mut out, "tags", &resolved.name);
    }
    if let Some(release_notes_url) = &resolved.release_notes_url {
        field(&mut out, "releaseNotes", release_notes_url);
    }
    if !resolved.dependencies.is_empty() {
        out.push_str("    <dependencies>\n");
        for dep in &resolved.dependencies {
            write!(out, "      <dependency id=\"{}\"", xml_escape(&dep.id))
                .expect("write to String cannot fail");
            if let Some(version) = &dep.version {
                write!(out, " version=\"{}\"", xml_escape(version))
                    .expect("write to String cannot fail");
            }
            out.push_str(" />\n");
        }
        out.push_str("    </dependencies>\n");
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

    let post_unzip = post_unzip_steps(resolved);
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
        for line in post_unzip.lines() {
            writeln!(out, "  {line}").expect("write to String cannot fail");
        }
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
        out.push_str(" `\n  -Checksum $checksum64 `\n  -ChecksumType 'sha256' `\n  -Checksum64 $checksum64 `\n  -ChecksumType64 'sha256'");
    }
    out.push('\n');
    out.push_str(&post_unzip);
    out
}

/// GUI shim markers and Start menu shortcuts, run after the zip is unpacked.
///
/// `<exe>.gui` makes Chocolatey's shimgen build a GUI shim, so the shim returns
/// at once instead of holding the console until the window closes. Shortcuts
/// go to the all-users Programs folder (`CommonPrograms`): Chocolatey installs
/// machine-wide, from an elevated shell, into `ProgramData`.
fn post_unzip_steps(resolved: &ResolvedChocolatey) -> String {
    let mut out = String::new();
    for exe in &resolved.gui_exes {
        let marker = ps_single_quoted(&format!("{}.gui", windows_path(exe)));
        out.push_str(&format!(
            "\nNew-Item -ItemType File -Force -Path (Join-Path $toolsDir '{marker}') | Out-Null\n"
        ));
    }
    if !resolved.shortcuts.is_empty() {
        out.push_str("\n$programs = [Environment]::GetFolderPath('CommonPrograms')\n");
    }
    for shortcut in &resolved.shortcuts {
        let lnk = ps_single_quoted(&format!("{}.lnk", shortcut.name));
        let target = ps_single_quoted(&windows_path(&shortcut.target));
        out.push_str(&format!(
            "Install-ChocolateyShortcut `\n  -ShortcutFilePath (Join-Path $programs '{lnk}') `\n  -TargetPath (Join-Path $toolsDir '{target}')"
        ));
        if let Some(arguments) = &shortcut.arguments {
            out.push_str(&format!(
                " `\n  -Arguments '{}'",
                ps_single_quoted(arguments)
            ));
        }
        if let Some(icon) = &shortcut.icon {
            out.push_str(&format!(
                " `\n  -IconLocation (Join-Path $toolsDir '{}')",
                ps_single_quoted(&windows_path(icon))
            ));
        }
        out.push('\n');
    }
    out
}

fn windows_path(path: &str) -> String {
    path.replace('/', "\\")
}

pub fn render_uninstall_script(opts: &ChocolateyRenderOptions<'_>) -> String {
    let shortcuts = &opts.resolved.shortcuts;
    if shortcuts.is_empty() {
        return "# No uninstall action is required for the default zip package layout.\n"
            .to_owned();
    }
    let mut out = String::from(
        "$ErrorActionPreference = 'Stop'\n\n$programs = [Environment]::GetFolderPath('CommonPrograms')\n",
    );
    for shortcut in shortcuts {
        out.push_str(&format!(
            "Remove-Item -LiteralPath (Join-Path $programs '{}') -Force -ErrorAction SilentlyContinue\n",
            ps_single_quoted(&format!("{}.lnk", shortcut.name))
        ));
    }
    out
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
    resolved.platform.release_download_url(
        &resolved.download_repo,
        &format!("{}{version}", resolved.tag_prefix.as_str()),
        &archive_file_name(resolved, version, arch),
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
    use super::*;
    use crate::cli::Platform;
    use crate::config::{ChocolateyPushConfig, ChocolateyShortcut};

    #[test]
    fn xml_escape_handles_special_chars() {
        assert_eq!(
            xml_escape("a & <b> \"c\" 'd'"),
            "a &amp; &lt;b&gt; &quot;c&quot; &apos;d&apos;"
        );
    }

    fn resolved() -> ResolvedChocolatey {
        ResolvedChocolatey {
            tag_prefix: crate::release_identity::TagPrefix::None,
            platform: Platform::Github,
            name: "sorrel".to_owned(),
            id: "sorrel".to_owned(),
            title: "Sorrel".to_owned(),
            authors: Some("Caniko".to_owned()),
            description: "Spike sorting curation".to_owned(),
            summary: None,
            project_url: "https://example.com".to_owned(),
            license_url: None,
            icon_url: None,
            package_source_url: None,
            docs_url: None,
            bug_tracker_url: None,
            project_source_url: None,
            tags: None,
            release_notes_url: None,
            download_repo: "caniko/sorrel".to_owned(),
            archive_pattern: "{name}-{version}-{arch}-windows.zip".to_owned(),
            push: ChocolateyPushConfig::default(),
            nix_tool: String::new(),
            api_key_env: String::new(),
            api_key_secret: String::new(),
            api_key_from_runner: false,
            dependencies: Vec::new(),
            shortcuts: Vec::new(),
            gui_exes: Vec::new(),
        }
    }

    fn scripts(resolved: &ResolvedChocolatey, include_x86: bool) -> (String, String) {
        let opts = ChocolateyRenderOptions {
            resolved,
            version: "1.2.3",
            include_x86,
        };
        (
            render_install_script(&opts, &Sha256Set::all_no_check()),
            render_uninstall_script(&opts),
        )
    }

    #[test]
    fn scripts_without_shortcuts_or_gui_exes_are_unchanged() {
        let (install, uninstall) = scripts(&resolved(), false);
        assert!(!install.contains("Shortcut") && !install.contains(".gui"));
        assert!(
            install.ends_with("  -UnzipLocation $toolsDir\n"),
            "{install}"
        );
        assert_eq!(
            uninstall,
            "# No uninstall action is required for the default zip package layout.\n"
        );
    }

    #[test]
    fn shortcuts_and_gui_exes_render_after_unzip_and_are_removed() {
        let resolved = ResolvedChocolatey {
            shortcuts: vec![
                ChocolateyShortcut {
                    name: "Sorrel".to_owned(),
                    target: "sorrelw.exe".to_owned(),
                    arguments: Some("--it's".to_owned()),
                    icon: Some("res/sorrel.ico".to_owned()),
                },
                ChocolateyShortcut {
                    name: "Sorrel CLI".to_owned(),
                    target: "sorrel.exe".to_owned(),
                    arguments: None,
                    icon: None,
                },
            ],
            gui_exes: vec!["sorrelw.exe".to_owned()],
            ..resolved()
        };
        let (install, uninstall) = scripts(&resolved, true);
        let unzip = install.rfind("Install-ChocolateyZipPackage").unwrap();
        // Both the 64-bit path and the 32-bit early return get the post-unzip steps.
        assert_eq!(install.matches("Install-ChocolateyShortcut").count(), 4);
        assert!(install.rfind("Install-ChocolateyShortcut").unwrap() > unzip);
        assert!(install.contains("[Environment]::GetFolderPath('CommonPrograms')"));
        assert!(install.contains(
            "New-Item -ItemType File -Force -Path (Join-Path $toolsDir 'sorrelw.exe.gui') | Out-Null"
        ));
        assert!(install.contains("-ShortcutFilePath (Join-Path $programs 'Sorrel.lnk')"));
        assert!(install.contains("-TargetPath (Join-Path $toolsDir 'sorrelw.exe')"));
        assert!(install.contains("-Arguments '--it''s'"), "{install}");
        assert!(install.contains("-IconLocation (Join-Path $toolsDir 'res\\sorrel.ico')"));
        assert!(install.contains("'Sorrel CLI.lnk'"));
        assert!(uninstall.contains(
            "Remove-Item -LiteralPath (Join-Path $programs 'Sorrel.lnk') -Force -ErrorAction SilentlyContinue"
        ));
        assert!(uninstall.contains("'Sorrel CLI.lnk'"));
    }
}
