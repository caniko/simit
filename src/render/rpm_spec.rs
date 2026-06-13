//! Render a Fedora/COPR RPM spec from resolved `[copr]` configuration.
//!
//! The spec vendors crates from `vendor.tar.gz` (Source1) so COPR builds work
//! without network access, mirroring the canonical layout produced by
//! `simit init copr` and the `.copr/Makefile` SRPM target.

use std::fmt::Write as _;

use crate::config::ResolvedCopr;

/// Render `<name>.spec` for the given release version.
pub fn render_spec(copr: &ResolvedCopr, version: &str) -> String {
    let mut out = String::new();

    writeln!(out, "%global crate {}", copr.name).expect("write");
    out.push('\n');
    field(&mut out, "Name", &copr.name);
    field(&mut out, "Version", version);
    field(&mut out, "Release", "1%{?dist}");
    field(&mut out, "Summary", &copr.summary);
    out.push('\n');
    field(&mut out, "License", &copr.license);
    field(&mut out, "URL", &copr.url);
    field(
        &mut out,
        "Source0",
        &format!(
            "%{{url}}/archive/v%{{version}}.tar.gz#/{}-v%{{version}}.tar.gz",
            copr.repo
        ),
    );
    field(&mut out, "Source1", "vendor.tar.gz");
    out.push('\n');
    for requirement in &copr.build_requires {
        field(&mut out, "BuildRequires", requirement);
    }
    out.push('\n');

    out.push_str("%description\n");
    out.push_str(copr.description.trim_end());
    out.push_str("\n\n");

    out.push_str("%prep\n");
    writeln!(out, "%autosetup -n {} -p1", copr.repo).expect("write");
    out.push_str("tar xf %{SOURCE1}\n");
    out.push_str("mkdir -p .cargo\n");
    out.push_str("cat > .cargo/config.toml << 'EOF'\n");
    out.push_str("[source.crates-io]\n");
    out.push_str("replace-with = \"vendored-sources\"\n\n");
    out.push_str("[source.vendored-sources]\n");
    out.push_str("directory = \"vendor\"\n");
    out.push_str("EOF\n\n");

    out.push_str("%build\n");
    out.push_str("cargo build --release --locked\n\n");

    out.push_str("%install\n");
    for binary in &copr.binaries {
        writeln!(
            out,
            "install -Dm755 target/release/{binary} %{{buildroot}}%{{_bindir}}/{binary}"
        )
        .expect("write");
    }
    out.push('\n');

    out.push_str("%files\n");
    out.push_str("%license LICENSE\n");
    out.push_str("%doc README.md CHANGELOG.md\n");
    for binary in &copr.binaries {
        writeln!(out, "%{{_bindir}}/{binary}").expect("write");
    }

    out
}

fn field(out: &mut String, tag: &str, value: &str) {
    writeln!(out, "{:<15} {value}", format!("{tag}:")).expect("write to String cannot fail");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ResolvedCopr;

    fn sample() -> ResolvedCopr {
        ResolvedCopr {
            name: "modde".to_owned(),
            summary: "Cross-platform game mod manager".to_owned(),
            description: "modde is a cross-platform game mod manager.".to_owned(),
            license: "GPL-3.0-only".to_owned(),
            url: "https://codeberg.org/caniko/rs-modde".to_owned(),
            download_repo: "caniko/rs-modde".to_owned(),
            repo: "rs-modde".to_owned(),
            build_requires: vec!["rust >= 1.85".to_owned(), "cargo".to_owned()],
            binaries: vec!["modde".to_owned(), "modde-ui".to_owned()],
            spec_path: "modde.spec".to_owned(),
            project: Some("caniko/rs-modde".to_owned()),
            testing_project: Some("caniko/rs-modde-testing".to_owned()),
            login_secret: "copr_login".to_owned(),
            username_secret: "copr_username".to_owned(),
            token_secret: "copr_token".to_owned(),
            nix_tool: "nixpkgs#copr-cli".to_owned(),
        }
    }

    #[test]
    fn renders_canonical_spec_fields() {
        let spec = render_spec(&sample(), "0.2.0");
        assert!(spec.contains("%global crate modde\n"));
        assert!(spec.contains("Name:           modde\n"));
        assert!(spec.contains("Version:        0.2.0\n"));
        assert!(spec.contains(
            "Source0:        %{url}/archive/v%{version}.tar.gz#/rs-modde-v%{version}.tar.gz\n"
        ));
        assert!(spec.contains("BuildRequires:  rust >= 1.85\n"));
        assert!(spec.contains("%autosetup -n rs-modde -p1\n"));
        assert!(
            spec.contains(
                "install -Dm755 target/release/modde-ui %{buildroot}%{_bindir}/modde-ui\n"
            )
        );
        assert!(spec.contains("%{_bindir}/modde\n"));
    }
}
