//! Native Cargo build jobs and their archive handoff to the release job.

use super::{
    ReleaseWorkflowInputs, VERSION_FROM_REF, download_artifact_action, forgejo_action_ref,
    github_action_ref, push_checkout, shell_single_quote,
};
use crate::cli::Platform;
use crate::config::{CargoReleaseArtifact, ExtractAssetConfig};
use crate::release_identity::TagPrefix;
use std::fmt::Write as _;

pub(super) fn push_job(
    w: &mut String,
    platform: Platform,
    prefix: TagPrefix,
    job: &CargoReleaseArtifact,
    not_republish: &str,
) {
    writeln!(w, "  cargo-{}:\n    needs: validate", job.id).expect("write");
    w.push_str(not_republish);
    if platform == Platform::Github {
        w.push_str("    permissions:\n      contents: read\n");
    }
    writeln!(
        w,
        "    runs-on: {}\n    timeout-minutes: 180\n    steps:",
        serde_json::to_string(&job.runner).expect("string")
    )
    .expect("write");
    push_checkout(w, platform, &job.runner, "${{ github.sha }}");
    if let Some(setup) = &job.setup {
        w.push_str("      - name: Prepare native build dependencies\n        shell: bash\n        run: |\n          set -euo pipefail\n");
        for line in setup.lines() {
            writeln!(w, "          {line}").expect("write");
        }
    }
    w.push_str("      - name: Build and archive Cargo binaries\n        id: archive\n        shell: bash\n        run: |\n          set -euo pipefail\n");
    writeln!(w, "          {VERSION_FROM_REF}").expect("write");
    if prefix == TagPrefix::V {
        w.push_str("          VERSION=\"${VERSION#v}\"\n");
    }
    writeln!(
        w,
        "          rustup toolchain install {toolchain} --profile minimal\n\
         {pad}host=\"$(rustc +{toolchain} -vV | sed -n 's/^host: //p')\"\n\
         {pad}test \"$host\" = '{target}' || {{ echo \"expected native Rust host {target}, got $host\" >&2; exit 1; }}",
        pad = "          ",
        toolchain = job.toolchain,
        target = job.target,
    )
    .expect("write");
    w.push_str("          mkdir -p target\n          stage=\"$(mktemp -d target/simit-cargo-release.XXXXXX)\"\n");
    write!(
        w,
        "          cargo +{} build --release --locked --target '{}' --target-dir \"$stage/build\"",
        job.toolchain, job.target
    )
    .expect("write");
    if !job.default_features {
        w.push_str(" --no-default-features");
    }
    for (flag, values) in [
        ("package", &job.packages),
        ("bin", &job.binaries),
        ("features", &job.features),
    ] {
        for value in values {
            write!(w, " --{flag}={}", shell_single_quote(value)).expect("write");
        }
    }
    w.push('\n');
    let archive = ExtractAssetConfig::shell_word(&job.archive);
    let directory = job.prefix.as_deref().map_or_else(String::new, |value| {
        format!("{}/", ExtractAssetConfig::shell_word(value))
    });
    let extension = if job.windows() { ".exe" } else { "" };
    writeln!(
        w,
        "          archive=\"{archive}\"\n          prefix=\"{directory}\"\n          mkdir -p \"$stage/files/$prefix\"\n          files=()"
    )
    .expect("write");
    w.push_str("          names=(");
    for name in &job.binaries {
        write!(w, "'{name}{extension}' ").expect("write");
    }
    writeln!(
        w,
        ")\n          for name in \"${{names[@]}}\"; do\n            binary=\"$stage/build/{}/release/$name\"\n            test -s \"$binary\"",
        job.target
    )
    .expect("write");
    if job.linux() {
        w.push_str(r#"            headers="$(LC_ALL=C readelf -lW "$binary")"
            dynamic="$(LC_ALL=C readelf -dW "$binary")"
            if grep -Eq '(Requesting program interpreter:|\((RPATH|RUNPATH)\)).*/nix/store' <<< "$headers"$'\n'"$dynamic"; then
              echo "::error::$name has an interpreter or library search path in /nix/store" >&2
              exit 1
            fi
"#);
    }
    w.push_str("            cp -p \"$binary\" \"$stage/files/$prefix$name\"\n            files+=(\"$prefix$name\")\n          done\n");
    let archive_command = match (job.windows(), job.archive.ends_with(".zip")) {
        (true, true) => {
            r#""$(cygpath -u "${SYSTEMROOT:-${SystemRoot:?}}")/System32/tar.exe" -a -cf"#
        }
        (true, false) => {
            r#""$(cygpath -u "${SYSTEMROOT:-${SystemRoot:?}}")/System32/tar.exe" -czf"#
        }
        (false, true) => "zip -q",
        (false, false) => "tar -czf",
    };
    writeln!(
        w,
        "          (cd \"$stage/files\" && {archive_command} \"../$archive\" -- \"${{files[@]}}\")\n\
         {pad}printf 'path=%s\\n' \"$stage/$archive\" >> \"$GITHUB_OUTPUT\"",
        pad = "          ",
    )
    .expect("write");
    let action = if platform == Platform::Forgejo {
        forgejo_action_ref("upload-artifact", "v4.6.2")
    } else {
        let version = if crate::render::ci::github_runner_supports_node24(&job.runner) {
            "v7.0.2"
        } else {
            "v4.6.2"
        };
        github_action_ref("actions/upload-artifact", version)
    };
    writeln!(
        w,
        "      - name: Upload Cargo release archive\n        uses: {action}\n        with:\n          name: simit-cargo-{}\n          path: ${{{{ steps.archive.outputs.path }}}}\n          if-no-files-found: error\n          retention-days: 1",
        job.id
    )
    .expect("write");
}

pub(super) fn push_collect(w: &mut String, inputs: &ReleaseWorkflowInputs<'_>) {
    let action = download_artifact_action(inputs.platform, inputs.runner);
    for job in &inputs.artifacts.cargo {
        writeln!(
            w,
            "      - name: Download Cargo archive ({id})\n        uses: {action}\n        with:\n          name: simit-cargo-{id}\n          path: ${{{{ runner.temp }}}}/simit-cargo-release/{id}",
            id = job.id,
        )
        .expect("write");
    }
    w.push_str(r#"      - name: Collect Cargo release archives
        run: |
          set -euo pipefail
          . ./release-env
          mkdir -p release
          collect() {
            local source="$RUNNER_TEMP/simit-cargo-release/$1/$2" dest="release/$2"
            test -s "$source" || { echo "Cargo release archive missing: $source" >&2; exit 1; }
            { test ! -e "$dest" && test ! -L "$dest"; } || { echo "release artifact collision: $dest" >&2; exit 1; }
            cp "$source" "$dest"
          }
"#);
    for job in &inputs.artifacts.cargo {
        writeln!(
            w,
            "          collect '{}' \"{}\"",
            job.id,
            ExtractAssetConfig::shell_word(&job.archive)
        )
        .expect("write");
    }
}
