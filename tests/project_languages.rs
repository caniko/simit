use std::fs;
use std::path::Path;
use std::process::Command;

use simit::project::detect_languages;
use tempfile::TempDir;

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn ignored_documentation_builds_do_not_change_language_policy() {
    let project = TempDir::new().unwrap();
    let root = project.path();
    git(root, &["init", "--quiet"]);
    fs::write(root.join(".gitignore"), "/docs/book/\n").unwrap();
    fs::write(root.join("README.md"), "# Demo\n").unwrap();
    let before = detect_languages(root).unwrap();

    fs::create_dir_all(root.join("docs/book")).unwrap();
    fs::write(root.join("docs/book/searcher.js"), "generated();\n").unwrap();
    assert_eq!(detect_languages(root).unwrap(), before);

    fs::create_dir_all(root.join("website")).unwrap();
    fs::write(root.join("website/app.ts"), "export {};\n").unwrap();
    assert!(detect_languages(root).unwrap().javascript);
}

#[test]
fn tracked_sources_override_ignore_rules_but_deleted_sources_do_not_count() {
    let project = TempDir::new().unwrap();
    let root = project.path();
    git(root, &["init", "--quiet"]);
    fs::write(root.join(".gitignore"), "*.js\n").unwrap();
    fs::write(root.join("app.js"), "source();\n").unwrap();
    git(root, &["add", "--force", "app.js"]);
    assert!(detect_languages(root).unwrap().javascript);

    fs::remove_file(root.join("app.js")).unwrap();
    assert!(!detect_languages(root).unwrap().javascript);
}

#[test]
fn non_git_projects_still_detect_nested_sources() {
    let project = TempDir::new().unwrap();
    fs::create_dir_all(project.path().join("website")).unwrap();
    fs::write(project.path().join("website/app.ts"), "export {};\n").unwrap();
    assert!(detect_languages(project.path()).unwrap().javascript);
}
