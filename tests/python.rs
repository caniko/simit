use std::fs;

fn project(metadata: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("pyproject.toml"), metadata).unwrap();
    root
}

#[test]
fn poetry_backed_dynamic_metadata_preserves_version_scripts_and_extras() {
    let root = project(
        r#"
[project]
name = "hybrid"
requires-python = ">=3.10,<3.13"
dynamic = ["version", "scripts", "optional-dependencies"]
[dependency-groups]
dev = ["pytest"]
[tool.poetry]
name = "legacy-name"
version = "1.2.3"
[tool.poetry.scripts]
hybrid = "hybrid.cli:main"
[tool.poetry.extras]
video = ["ffmpeg-python"]
[tool.poetry.group.legacy.dependencies]
black = "*"
"#,
    );
    let parsed = simit::python::load_project(root.path()).unwrap();
    assert_eq!(parsed.name, "hybrid");
    assert_eq!(parsed.version, "1.2.3");
    assert_eq!(parsed.requires_python.as_deref(), Some(">=3.10,<3.13"));
    assert_eq!(parsed.scripts, ["hybrid"]);
    assert_eq!(parsed.optional_extras, ["video"]);
    assert_eq!(parsed.dependency_groups, ["dev"]);
}

#[test]
fn static_project_metadata_takes_precedence_over_poetry() {
    let root = project(
        r#"
[project]
name = "modern"
version = "2.0.0"
[project.scripts]
modern = "modern:main"
[project.optional-dependencies]
gpu = ["torch"]
[tool.poetry]
name = "legacy"
version = "1.0.0"
[tool.poetry.scripts]
legacy = "legacy:main"
[tool.poetry.extras]
video = ["ffmpeg-python"]
"#,
    );
    let parsed = simit::python::load_project(root.path()).unwrap();
    assert_eq!(parsed.version, "2.0.0");
    assert_eq!(parsed.scripts, ["modern"]);
    assert_eq!(parsed.optional_extras, ["gpu"]);
}

#[test]
fn absent_static_fields_do_not_implicitly_inherit_poetry_metadata() {
    let root = project(
        r#"
[project]
name = "modern"
version = "2.0.0"
[tool.poetry]
name = "legacy"
version = "1.0.0"
[tool.poetry.scripts]
legacy = "legacy:main"
[tool.poetry.extras]
video = ["ffmpeg-python"]
"#,
    );
    let parsed = simit::python::load_project(root.path()).unwrap();
    assert!(parsed.scripts.is_empty());
    assert!(parsed.optional_extras.is_empty());
}

#[test]
fn dynamic_version_without_a_known_provider_has_an_actionable_error() {
    let root = project("[project]\nname = 'dynamic'\ndynamic = ['version']\n");
    let error = simit::python::load_project(root.path()).unwrap_err();
    assert!(error.to_string().contains("dynamic version"), "{error:#}");
}

#[test]
fn missing_version_without_dynamic_declaration_is_rejected() {
    let root = project(
        "[project]\nname = 'incomplete'\n[tool.poetry]\nname = 'legacy'\nversion = '1.0.0'\n",
    );
    assert!(simit::python::load_project(root.path()).is_err());
}
