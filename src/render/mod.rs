pub mod apt_conf;
pub mod chocolatey_nuspec;
pub mod ci;
pub mod copr_makefile;
pub mod crow;
pub mod diff;
pub mod flake;
pub mod gitignore;
pub mod homebrew_formula;
pub mod pkgbuild;
pub mod release_plz;
pub mod release_workflow;
pub mod review_policy;
pub mod rpm_spec;
pub mod scoop_manifest;
pub mod workflow_templates;

const SIMIT_GIT: &str = "https://github.com/caniko/simit.git";

/// `simit init flake --check --diff` pinned to this generator.
pub fn flake_wiring_check() -> String {
    format!(
        "nix run --no-write-lock-file {} -- init flake --check --diff",
        simit_flake_ref(source_revision())
    )
}

/// Windows packager `cargo install` pinned to this generator.
pub fn simit_cargo_install() -> String {
    simit_cargo_install_at(source_revision())
}

/// The flake package's clean `self.rev`. Cargo installs and dirty trees carry
/// none, so their generated references pin the crate version's release tag.
fn source_revision() -> Option<&'static str> {
    revision_pin(option_env!("SIMIT_SOURCE_REVISION"))
}

// Only a full commit id pins; `dirtyRev` and anything else fall back to the tag.
fn revision_pin(raw: Option<&str>) -> Option<&str> {
    raw.filter(|rev| crate::release_publisher::is_sha(rev))
}

fn simit_flake_ref(revision: Option<&str>) -> String {
    match revision {
        Some(rev) => format!("git+{SIMIT_GIT}?rev={rev}"),
        None => format!(
            "git+{SIMIT_GIT}?ref=refs/tags/{}",
            env!("CARGO_PKG_VERSION")
        ),
    }
}

fn simit_cargo_install_at(revision: Option<&str>) -> String {
    match revision {
        Some(rev) => format!("cargo install --locked --git {SIMIT_GIT} --rev {rev} simit"),
        None => format!(
            "cargo install --locked simit --version ={}",
            env!("CARGO_PKG_VERSION")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_simit_references_pin_the_generator() {
        let rev = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            simit_flake_ref(revision_pin(Some(rev))),
            format!("git+{SIMIT_GIT}?rev={rev}")
        );
        assert_eq!(
            simit_cargo_install_at(revision_pin(Some(rev))),
            format!("cargo install --locked --git {SIMIT_GIT} --rev {rev} simit")
        );

        let version = env!("CARGO_PKG_VERSION");
        let dirty = format!("{rev}-dirty");
        let upper = rev.to_uppercase();
        for unpinned in [
            None,
            Some(""),
            Some("0123456-dirty"),
            Some(dirty.as_str()),
            Some(upper.as_str()),
        ] {
            assert_eq!(
                simit_flake_ref(revision_pin(unpinned)),
                format!("git+{SIMIT_GIT}?ref=refs/tags/{version}")
            );
            assert_eq!(
                simit_cargo_install_at(revision_pin(unpinned)),
                format!("cargo install --locked simit --version ={version}")
            );
        }
    }
}
