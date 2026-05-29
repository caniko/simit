//! Render `.copr/Makefile` — the COPR SCM `srpm` target.
//!
//! COPR clones the repository and runs `make -f .copr/Makefile srpm`, which
//! downloads the release tarball, vendors crates offline, and builds the SRPM
//! that COPR then rebuilds for each chroot.

use std::fmt::Write as _;

use crate::config::ResolvedCopr;

/// Render the `.copr/Makefile` contents.
pub fn render_makefile(copr: &ResolvedCopr) -> String {
    let repo = &copr.repo;
    let spec = &copr.spec_path;
    let mut out = String::new();
    out.push_str(".PHONY: srpm\n\n");
    writeln!(
        out,
        "VERSION = $(shell grep '^Version:' {spec} | awk '{{print $$2}}')"
    )
    .expect("write");
    out.push('\n');
    out.push_str("srpm:\n");
    writeln!(out, "\tcurl -Lo {repo}-v$(VERSION).tar.gz \\").expect("write");
    writeln!(
        out,
        "\t\t\"https://codeberg.org/{}/archive/v$(VERSION).tar.gz\"",
        copr.download_repo
    )
    .expect("write");
    writeln!(out, "\ttar xf {repo}-v$(VERSION).tar.gz").expect("write");
    writeln!(out, "\tcd {repo} && cargo vendor").expect("write");
    writeln!(out, "\ttar czf vendor.tar.gz -C {repo} vendor").expect("write");
    writeln!(out, "\trpmbuild -bs {spec} \\").expect("write");
    out.push_str("\t\t--define \"_sourcedir $(CURDIR)\" \\\n");
    out.push_str("\t\t--define \"_srcrpmdir $(outdir)\"\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ResolvedCopr {
        ResolvedCopr {
            name: "modde".to_owned(),
            summary: "x".to_owned(),
            description: "x".to_owned(),
            license: "GPL-3.0-only".to_owned(),
            url: "https://codeberg.org/caniko/rs-modde".to_owned(),
            download_repo: "caniko/rs-modde".to_owned(),
            repo: "rs-modde".to_owned(),
            build_requires: vec![],
            binaries: vec!["modde".to_owned()],
            spec_path: "modde.spec".to_owned(),
            project: None,
            testing_project: None,
            login_secret: "copr_login".to_owned(),
            username_secret: "copr_username".to_owned(),
            token_secret: "copr_token".to_owned(),
        }
    }

    #[test]
    fn renders_srpm_target() {
        let makefile = render_makefile(&sample());
        assert!(
            makefile.contains("VERSION = $(shell grep '^Version:' modde.spec | awk '{print $$2}')")
        );
        assert!(makefile.contains("\tcurl -Lo rs-modde-v$(VERSION).tar.gz \\\n"));
        assert!(
            makefile.contains(
                "\t\t\"https://codeberg.org/caniko/rs-modde/archive/v$(VERSION).tar.gz\"\n"
            )
        );
        assert!(makefile.contains("\tcd rs-modde && cargo vendor\n"));
        assert!(makefile.contains("\trpmbuild -bs modde.spec \\\n"));
    }
}
