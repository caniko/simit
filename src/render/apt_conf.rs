//! Render the reprepro `conf/distributions` file for the apt repository.
//!
//! `simit init apt` writes this under `dist/apt/conf/distributions`. The
//! generated release workflow copies it into a throwaway reprepro tree that
//! contains exactly one imported signing key, so `SignWith: yes` selects it.

use std::fmt::Write as _;

use crate::config::ResolvedApt;

/// Render `dist/apt/conf/distributions`.
pub fn render_distributions(apt: &ResolvedApt) -> String {
    let mut out = String::new();
    writeln!(out, "Origin: {}", apt.label).expect("write");
    writeln!(out, "Label: {}", apt.label).expect("write");
    writeln!(out, "Codename: {}", apt.distribution).expect("write");
    writeln!(out, "Architectures: {}", apt.architectures).expect("write");
    writeln!(out, "Components: {}", apt.components).expect("write");
    writeln!(out, "Description: {} apt repository", apt.label).expect("write");
    out.push_str("SignWith: yes\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ResolvedApt {
        ResolvedApt {
            label: "modde".to_owned(),
            repo_url: "ssh://git@codeberg.org/caniko/rs-modde-apt.git".to_owned(),
            public_url: None,
            pages_provider: "codeberg-git-pages".to_owned(),
            pages_runner: None,
            branch: "pages".to_owned(),
            distribution: "stable".to_owned(),
            architectures: "amd64".to_owned(),
            components: "main".to_owned(),
            debian_release: "bookworm".to_owned(),
            packages: vec!["modde-cli".to_owned()],
            build_deps: vec!["gcc".to_owned()],
            cargo_deb_version: "2.5.0".to_owned(),
            gpg_key_secret: "modde_apt_repo_gpg_key".to_owned(),
            gpg_key_id_secret: "modde_apt_repo_gpg_key_id".to_owned(),
            gpg_passphrase_secret: "modde_apt_repo_gpg_passphrase".to_owned(),
            ssh_key_secret: "modde_apt_repo_ssh_key".to_owned(),
        }
    }

    #[test]
    fn renders_distributions() {
        let conf = render_distributions(&sample());
        assert!(conf.contains("Codename: stable\n"));
        assert!(conf.contains("Architectures: amd64\n"));
        assert!(conf.contains("Components: main\n"));
        assert!(conf.contains("SignWith: yes\n"));
    }
}
