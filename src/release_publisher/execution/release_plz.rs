//! Data-only admission for release-plz's same-run, credential-free archives.

use super::*;

pub(super) fn run(command: ReleaseCommand) -> Result<()> {
    let stage = command
        .publisher_stage
        .context("--publisher-stage is required")?;
    let expected = env_value("SIMIT_EXPECTED_RELEASE_SHA")?;
    ensure!(
        expected.len() == 40 && expected.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid qualified source SHA"
    );
    let repository = env_value("GITHUB_REPOSITORY")?;
    ensure!(
        command.secrets_repo.as_deref() == Some(repository.as_str()),
        "publisher repository mismatch"
    );
    let event: Value = serde_json::from_slice(&fs::read(env_value("GITHUB_EVENT_PATH")?)?)?;
    ensure!(
        env_value("GITHUB_EVENT_NAME")? == "push"
            && event["repository"]["full_name"] == repository
            && env_value("GITHUB_REF")?
                == format!(
                    "refs/heads/{}",
                    event["repository"]["default_branch"]
                        .as_str()
                        .context("missing default branch")?
                ),
        "release-plz publication requires the native default-branch push"
    );
    let state = TempDir::new()?;
    for directory in ["home", "cargo-home", "gpg"] {
        fs::create_dir(state.path().join(directory))?;
    }
    let workspace = env_value("SIMIT_RELEASE_PLZ_SOURCE")?;
    let event_sha = env_value("GITHUB_SHA")?;
    let head = output(
        data_command("git", state.path())?
            .arg("-C")
            .arg(&workspace)
            .args(["rev-parse", "HEAD"]),
    )?;
    ensure!(
        String::from_utf8(head.stdout)?.trim() == event_sha,
        "publisher requires the immutable event checkout"
    );
    output(
        data_command("git", state.path())?
            .arg("-C")
            .arg(&workspace)
            .args(["merge-base", "--is-ancestor", &expected, &event_sha]),
    )?;
    let source = state.path().join("source");
    output(
        data_command("git", state.path())?
            .args(["clone", "--no-hardlinks", "--no-checkout", "--"])
            .arg(&workspace)
            .arg(&source),
    )?;
    checkout_signed_source(state.path(), &source, &expected)?;
    let tracked = output(
        data_command("git", state.path())?
            .arg("-C")
            .arg(&source)
            .args(["ls-files", "--stage", "-z"]),
    )?;
    let links = super::super::source::validate_tree(&source, &tracked.stdout)?;
    validate_manifest_paths_with_links(&source, &links)?;
    // A neutral cwd excludes project Cargo config, wrappers and credential providers.
    let metadata = output(
        data_command("cargo", state.path())?
            .current_dir("/")
            .args([
                "metadata",
                "--no-deps",
                "--format-version",
                "1",
                "--manifest-path",
            ])
            .arg(source.join("Cargo.toml")),
    )?;
    let metadata: crate::cargo::Metadata = serde_json::from_slice(&metadata.stdout)?;
    let plan = crate::commands::release_plan::build_release_plan(&metadata, &[])?;
    let names: Vec<_> = plan
        .entries
        .iter()
        .map(|entry| entry.package.name.clone())
        .collect();
    ensure!(!names.is_empty(), "release-plz has no publishable packages");
    ensure!(
        stage == PublisherStage::Prepare
            || (command.packages.len() == 1 && names.contains(&command.packages[0])),
        "publication requires one qualified workspace package"
    );
    // ponytail: reconstruct the workspace for each upload (O(n²) packages).
    // A future sealed same-worker admission receipt can reuse this reconstruction.
    output(&mut reconstruction_command(state.path(), &source, &names)?)?;
    let archives = command
        .release_plz_archives
        .context("missing release-plz archive directory")?;
    let mut selected = None;
    for entry in &plan.entries {
        let package = &entry.package;
        ensure!(
            package.is_publishable_to_crates_io(),
            "only crates.io publication is supported"
        );
        let root = format!("{}-{}", package.name, package.version);
        let archive = archives.as_std_path().join(format!("{root}.crate"));
        ensure!(
            fs::symlink_metadata(&archive)?.file_type().is_file()
                && fs::metadata(&archive)?.len() <= MAX_CRATE_BYTES,
            "invalid candidate archive"
        );
        let candidate = fs::read(archive)?;
        let reconstructed = fs::read(
            state
                .path()
                .join("target/package")
                .join(format!("{root}.crate")),
        )?;
        verify_reconstructed_crate(&candidate, &reconstructed, &root)?;
        let members = crate_members(&candidate, &root)?;
        super::super::source::reject_link_members(&members, &root, &source, &links)?;
        let vcs: Value = serde_json::from_slice(
            &members
                .get(&format!("{root}/.cargo_vcs_info.json"))
                .context("missing crate source binding")?
                .contents,
        )?;
        ensure!(
            vcs["git"]["sha1"] == expected,
            "candidate archive source differs from qualified revision"
        );
        let manifest: toml_edit::DocumentMut = std::str::from_utf8(
            &members
                .get(&format!("{root}/Cargo.toml"))
                .context("missing normalized manifest")?
                .contents,
        )?
        .parse()?;
        let upload = upload_metadata(&manifest, &members, &root, &package.name, &package.version)?;
        if command.packages.first() == Some(&package.name) {
            selected = Some((upload, candidate));
        }
    }
    if stage == PublisherStage::Publish {
        let (metadata, archive) = selected.context("selected archive is missing")?;
        let checksum = hex::encode(Sha256::digest(&archive));
        let client = Client::builder()
            .user_agent("simit-release-plz-publisher")
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()?;
        publish_archive(
            &client,
            env::var("CRATES_IO_API_TOKEN").ok().as_deref(),
            &metadata,
            &archive,
            &checksum,
        )?;
        wait_for_registry(
            &client,
            state.path(),
            metadata["name"].as_str().context("missing name")?,
            metadata["vers"].as_str().context("missing version")?,
            &checksum,
        )?;
    }
    Ok(())
}
