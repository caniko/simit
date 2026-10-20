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
    let client = Client::builder()
        .user_agent("simit-release-plz-publisher")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?;
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
        // Check *every* plan entry before selecting an upload. release-plz skips
        // existing versions without invoking our Cargo publisher for them.
        if (stage == PublisherStage::Publish || command.release_plz_check_registry)
            && let Some(record) = registry_record(&client, &package.name, &package.version)?
        {
            reconcile_existing_archive(
                &package.name,
                &package.version,
                &candidate,
                &record,
                || {
                    let mut bytes = Vec::new();
                    client
                        .get(format!(
                            "https://static.crates.io/crates/{}/{root}.crate",
                            package.name
                        ))
                        .send()?
                        .error_for_status()?
                        .take(MAX_CRATE_BYTES + 1)
                        .read_to_end(&mut bytes)?;
                    ensure!(
                        bytes.len() as u64 <= MAX_CRATE_BYTES,
                        "published crate exceeds byte limit"
                    );
                    Ok(bytes)
                },
                |sha| {
                    output(
                        data_command("git", state.path())?
                            .arg("-C")
                            .arg(&source)
                            .args(["merge-base", "--is-ancestor", sha, &expected]),
                    )?;
                    Ok(())
                },
            )?;
        }
        if command.packages.first() == Some(&package.name) {
            selected = Some((upload, candidate));
        }
    }
    if stage == PublisherStage::Publish {
        let (metadata, archive) = selected.context("selected archive is missing")?;
        let checksum = hex::encode(Sha256::digest(&archive));
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

fn reconcile_existing_archive(
    name: &str,
    version: &str,
    candidate: &[u8],
    record: &Value,
    download: impl FnOnce() -> Result<Vec<u8>>,
    ancestor: impl FnOnce(&str) -> Result<()>,
) -> Result<()> {
    let checksum = record["version"]["checksum"]
        .as_str()
        .context("missing registry checksum")?;
    ensure!(
        checksum.len() == 64
            && checksum.bytes().all(|byte| byte.is_ascii_hexdigit())
            && exact_registry_record(record, name, version, checksum),
        "registry conflict, yanked crate or invalid checksum for {name}@{version}"
    );
    if checksum == hex::encode(Sha256::digest(candidate)) {
        return Ok(());
    }
    let published = download()?;
    ensure!(
        hex::encode(Sha256::digest(&published)) == checksum,
        "published archive checksum mismatch"
    );
    let root = format!("{name}-{version}");
    let mut published = crate_members(&published, &root)?;
    let candidate = crate_members(candidate, &root)?;
    let vcs = format!("{root}/.cargo_vcs_info.json");
    let current = candidate
        .get(&vcs)
        .context("candidate has no source binding")?;
    let previous = published
        .get_mut(&vcs)
        .context("published crate has no source binding")?;
    let mut provenance: Value = serde_json::from_slice(&previous.contents)?;
    let expected: Value = serde_json::from_slice(&current.contents)?;
    let sha = provenance["git"]["sha1"]
        .as_str()
        .context("published crate has no source SHA")?;
    ensure!(
        sha.len() == 40
            && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
            && provenance["git"]
                .get("dirty")
                .is_none_or(|dirty| dirty == false),
        "invalid published source binding"
    );
    ancestor(sha)?;
    // Unchanged workspace members have an older VCS SHA, not identical archive
    // bytes. Only that SHA may differ: all package data, modes and other source
    // binding fields must still match the qualified reconstruction.
    provenance["git"]["sha1"] = expected["git"]["sha1"].clone();
    ensure!(
        provenance == expected,
        "published crate source binding differs"
    );
    previous.contents.clone_from(&current.contents);
    super::super::archive::verify_crate_members(&published, &candidate, &root)
        .with_context(|| format!("registry crate {name}@{version} differs from qualified contents"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(sha: &str, code: &str, dirty: bool) -> Vec<u8> {
        use std::io::Write;
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        for (path, contents) in [
            (
                "Cargo.toml",
                "[package]\nname='base'\nversion='0.1.0'\n".to_owned(),
            ),
            ("src/lib.rs", code.to_owned()),
            (
                ".cargo_vcs_info.json",
                json!({"git":{"sha1":sha,"dirty":dirty},"path_in_vcs":"base"}).to_string(),
            ),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(contents.len() as u64);
            header.set_cksum();
            tar.append_data(
                &mut header,
                format!("base-0.1.0/{path}"),
                contents.as_bytes(),
            )
            .unwrap();
        }
        let mut gzip = tar.into_inner().unwrap();
        gzip.flush().unwrap();
        gzip.finish().unwrap()
    }

    fn record(archive: &[u8]) -> Value {
        json!({"version":{"crate":"base","num":"0.1.0","yanked":false,"checksum":hex::encode(Sha256::digest(archive))}})
    }

    #[test]
    fn skipped_workspace_crates_must_match_the_qualified_contents() {
        let candidate = archive(&"f".repeat(40), "pub fn trusted() {}", false);
        let old_sha = "a".repeat(40);
        let published = archive(&old_sha, "pub fn trusted() {}", false);
        reconcile_existing_archive(
            "base",
            "0.1.0",
            &candidate,
            &record(&published),
            || Ok(published.clone()),
            |sha| {
                ensure!(sha == old_sha, "non-ancestor");
                Ok(())
            },
        )
        .unwrap();
        for published in [
            archive(&old_sha, "pub fn injected() {}", false),
            archive(&old_sha, "pub fn trusted() {}", true),
            archive(&"b".repeat(40), "pub fn trusted() {}", false),
        ] {
            assert!(
                reconcile_existing_archive(
                    "base",
                    "0.1.0",
                    &candidate,
                    &record(&published),
                    || Ok(published),
                    |sha| {
                        ensure!(sha == old_sha, "non-ancestor");
                        Ok(())
                    }
                )
                .is_err()
            );
        }
    }

    #[test]
    fn registry_reconciliation_checks_identity_checksum_and_yanks_before_side_effects() {
        let candidate = archive(&"f".repeat(40), "pub fn trusted() {}", false);
        let valid = record(&candidate);
        reconcile_existing_archive(
            "base",
            "0.1.0",
            &candidate,
            &valid,
            || panic!("identical archive must not need download"),
            |_| panic!("already bound to qualified source"),
        )
        .unwrap();
        for field in ["crate", "num", "checksum", "yanked"] {
            let mut bad = valid.clone();
            bad["version"][field] = if field == "yanked" {
                json!(true)
            } else {
                json!("wrong")
            };
            assert!(
                reconcile_existing_archive(
                    "base",
                    "0.1.0",
                    &candidate,
                    &bad,
                    || panic!("invalid identity must fail before download"),
                    |_| unreachable!()
                )
                .is_err()
            );
        }
        let published = archive(&"a".repeat(40), "pub fn trusted() {}", false);
        assert!(
            reconcile_existing_archive(
                "base",
                "0.1.0",
                &candidate,
                &record(&published),
                || Ok(candidate.clone()),
                |_| unreachable!()
            )
            .is_err(),
            "download must match the registry checksum"
        );
    }
}
