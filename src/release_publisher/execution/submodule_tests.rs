//! Local-fixture admission of signed-tree-pinned submodules.
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::super::source::{
    Gitlinks, admit_submodules, github_owner_prefix, reject_link_members, validate_signed_tree,
    validate_tree,
};
use super::*;

const CRATE: &str = "simit-submodule-fixture";

fn git(state: &Path, directory: &Path, args: &[&str]) -> String {
    // Operator-authorized disposable fixture identity; never release metadata.
    let result = output(
        data_command("git", state)
            .unwrap()
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "protocol.file.allow=always",
                "-C",
            ])
            .arg(directory)
            .args(args),
    )
    .unwrap();
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn commit(state: &Path, directory: &Path) -> String {
    git(state, directory, &["commit", "-q", "-m", "fixture"]);
    git(state, directory, &["rev-parse", "HEAD"])
}

struct Fixture {
    state: TempDir,
    /// `file://` stand-in for `https://github.com/<owner>/`.
    prefix: String,
    superproject: PathBuf,
    submodule: String,
}

impl Fixture {
    /// A remote crate repository; `nested` adds a gitlink inside it.
    fn new(nested: bool) -> Self {
        let state = TempDir::new().unwrap();
        let remotes = state.path().join("remotes");
        let sub = remotes.join("sub");
        output(
            data_command("git", state.path())
                .unwrap()
                .arg("init")
                .arg(&sub),
        )
        .unwrap();
        fs::create_dir(sub.join("src")).unwrap();
        fs::write(sub.join("src/lib.rs"), "pub fn pinned() {}\n").unwrap();
        fs::write(sub.join("Cargo.toml"), format!("[package]\nname='{CRATE}'\nversion='1.0.0'\nedition='2021'\nlicense='MIT'\ndescription='fixture'\n")).unwrap();
        git(state.path(), &sub, &["add", "."]);
        let mut submodule = commit(state.path(), &sub);
        if nested {
            let record = format!("160000,{submodule},inner");
            git(
                state.path(),
                &sub,
                &["update-index", "--add", "--cacheinfo", &record],
            );
            submodule = commit(state.path(), &sub);
        }
        let superproject = remotes.join("super");
        output(
            data_command("git", state.path())
                .unwrap()
                .arg("init")
                .arg(&superproject),
        )
        .unwrap();
        fs::write(
            superproject.join("Cargo.toml"),
            "[workspace]\nmembers=['components/sub']\nresolver='2'\n",
        )
        .unwrap();
        Fixture {
            prefix: format!("file://{}/", remotes.display()),
            state,
            superproject,
            submodule,
        }
    }

    fn url(&self) -> String {
        format!("{}sub", self.prefix)
    }

    /// Commit the superproject with `gitmodules` and optionally the gitlink,
    /// then return the data-only signed-source checkout of that commit.
    fn signed(&self, gitmodules: Option<&str>, gitlink: bool) -> (PathBuf, String) {
        let state = self.state.path();
        let root = &self.superproject;
        if let Some(gitmodules) = gitmodules {
            fs::write(root.join(".gitmodules"), gitmodules).unwrap();
            git(state, root, &["add", ".gitmodules"]);
        }
        if gitlink {
            let record = format!("160000,{},components/sub", self.submodule);
            git(
                state,
                root,
                &["update-index", "--add", "--cacheinfo", &record],
            );
        }
        git(state, root, &["add", "Cargo.toml"]);
        let revision = commit(state, root);
        let source = state.join("source");
        output(data_command("git", state).unwrap().arg("init").arg(&source)).unwrap();
        git(
            state,
            &source,
            &["fetch", "-q", root.to_str().unwrap(), &revision],
        );
        checkout_signed_source(state, &source, &revision).unwrap();
        (source, revision)
    }

    fn declared(&self) -> String {
        format!(
            "[submodule \"sub\"]\n\tpath = components/sub\n\turl = {}\n",
            self.url()
        )
    }

    fn admit(&self, source: &Path, revision: &str) -> Result<super::super::source::Submodules> {
        let state = self.state.path().to_owned();
        admit_submodules(
            &move || data_command("git", &state),
            source,
            revision,
            &self.prefix,
        )
    }
}

fn tracked(state: &Path, source: &Path) -> Vec<u8> {
    output(
        data_command("git", state)
            .unwrap()
            .arg("-C")
            .arg(source)
            .args(["ls-files", "--stage", "-z"]),
    )
    .unwrap()
    .stdout
}

fn error(result: Result<impl std::fmt::Debug>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn pinned_submodule_crate_is_admitted_and_reconstructs_the_candidate() {
    let fixture = Fixture::new(false);
    let state = fixture.state.path();
    let root = &fixture.superproject;
    fs::write(root.join(".gitmodules"), fixture.declared()).unwrap();
    let record = format!("160000,{},components/sub", fixture.submodule);
    git(
        state,
        root,
        &["update-index", "--add", "--cacheinfo", &record],
    );
    // The candidate side mirrors a recursive actions/checkout of the tag.
    git(
        state,
        root,
        &["submodule", "update", "--init", "-q", "components/sub"],
    );
    output(
        data_command("cargo", state)
            .unwrap()
            .args(["generate-lockfile", "--offline", "--manifest-path"])
            .arg(root.join("Cargo.toml")),
    )
    .unwrap();
    git(state, root, &["add", "Cargo.lock"]);
    let (source, revision) = fixture.signed(Some(&fixture.declared()), true);
    let packages = [CRATE.to_owned()];
    let archive_root = format!("{CRATE}-1.0.0");
    let archive = state.join(format!("target/package/{archive_root}.crate"));
    output(
        reconstruction_command(state, root, &packages)
            .unwrap()
            .arg("--offline"),
    )
    .unwrap();
    let candidate = fs::read(&archive).unwrap();

    // Without the setting the signed gitlink stays unsupported.
    assert!(validate_tree(&source, &tracked(state, &source)).is_err());
    let submodules = fixture.admit(&source, &revision).unwrap();
    assert_eq!(
        submodules.gitlinks,
        Gitlinks::from([(PathBuf::from("components/sub"), fixture.submodule.clone())])
    );
    let mut links =
        validate_signed_tree(&source, &tracked(state, &source), &submodules.gitlinks).unwrap();
    links.extend(submodules.links);
    validate_manifest_paths_with_links(&source, &links).unwrap();
    output(
        reconstruction_command(state, &source, &packages)
            .unwrap()
            .arg("--offline"),
    )
    .unwrap();
    let reconstructed = fs::read(&archive).unwrap();
    verify_reconstructed_crate(&candidate, &reconstructed, &archive_root).unwrap();
    let members = crate_members(&candidate, &archive_root).unwrap();
    reject_link_members(
        &members,
        &archive_root,
        &source,
        &links,
        &submodules.gitlinks,
    )
    .unwrap();
    // A Git symlink blob inside the submodule crate is still a rejected member.
    let link = source.join("components/sub/src/lib.rs");
    assert!(
        reject_link_members(
            &members,
            &archive_root,
            &source,
            &[link].into(),
            &submodules.gitlinks
        )
        .is_err()
    );
    // An admitted gitlink whose index id differs from the admitted id is rejected.
    let foreign = Gitlinks::from([(PathBuf::from("components/sub"), "0".repeat(40))]);
    assert!(validate_signed_tree(&source, &tracked(state, &source), &foreign).is_err());
}

#[test]
fn undeclared_gitlinks_and_gitlink_free_declarations_are_rejected() {
    for (declared, gitlink) in [(false, true), (true, false)] {
        let fixture = Fixture::new(false);
        let gitmodules = declared.then(|| fixture.declared());
        let (source, revision) = fixture.signed(gitmodules.as_deref(), gitlink);
        assert!(
            error(fixture.admit(&source, &revision))
                .contains("exactly one .gitmodules declaration"),
            "{declared} {gitlink}"
        );
    }
    // A declaration for another path does not cover the gitlink either.
    let fixture = Fixture::new(false);
    let gitmodules = fixture
        .declared()
        .replace("components/sub", "components/other");
    let (source, revision) = fixture.signed(Some(&gitmodules), true);
    assert!(error(fixture.admit(&source, &revision)).contains("exactly one"));
}

#[test]
fn foreign_owner_and_host_urls_are_rejected_before_fetching() {
    for url in [
        "https://github.com/someone-else/sub",
        "https://gitlab.com/caniko/sub",
    ] {
        let fixture = Fixture::new(false);
        let gitmodules = fixture.declared().replace(&fixture.url(), url);
        let (source, revision) = fixture.signed(Some(&gitmodules), true);
        let state = fixture.state.path().to_owned();
        let result = admit_submodules(
            &move || data_command("git", &state),
            &source,
            &revision,
            &github_owner_prefix("caniko/harbor").unwrap(),
        );
        assert!(
            error(result).contains("HTTPS repository of the superproject owner"),
            "{url}"
        );
        assert!(
            fs::read_dir(source.join("components/sub"))
                .unwrap()
                .next()
                .is_none()
        );
    }
}

#[test]
fn nested_submodules_are_rejected() {
    let fixture = Fixture::new(true);
    let (source, revision) = fixture.signed(Some(&fixture.declared()), true);
    assert!(error(fixture.admit(&source, &revision)).contains("unsupported entry or submodule"));
}

#[test]
fn unsupported_gitmodules_keys_are_rejected() {
    let fixture = Fixture::new(false);
    let gitmodules = format!("{}\tshallow = true\n", fixture.declared());
    let (source, revision) = fixture.signed(Some(&gitmodules), true);
    assert!(error(fixture.admit(&source, &revision)).contains("unsupported .gitmodules key"));
}
