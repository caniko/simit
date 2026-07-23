//! Generate the comprehensive single-job release workflow (`release.yml`).
//!
//! This reproduces a hand-rolled Forgejo/Codeberg release pipeline generically:
//! validate the signed tag, build artifacts (project-supplied build commands
//! plus generic SRPM/deb builds), sign checksums, create the Codeberg release,
//! and publish to every configured distribution channel. Each channel step is
//! gated on its config being present and (for downstream package repos) on the
//! release being stable.
//!
//! The project-specific platform build (cross-compiled binaries, AppImages,
//! Flatpak manifest, ...) is supplied verbatim via
//! `[release.artifacts].build_commands`; simit owns the standardized scaffolding
//! and the channel publish steps.

use std::fmt::Write as _;

use crate::cli::Platform;
use crate::config::{
    AnnounceConfig, ArtifactsConfig, AtticConfig, FlatpakConfig, ReleaseProvider,
    ReleasePublisherEnforcement, ResolvedApt, ResolvedAur, ResolvedChocolatey, ResolvedCopr,
    ResolvedHomebrew, ResolvedReleaseTarget, ResolvedScoop, WindowsSigningConfig, WingetConfig,
};
use crate::render::ci::{forgejo_action_ref, github_action_ref, immutable_action_ref};
use serde::Serialize;

/// Everything the release workflow generator needs, resolved up front.
pub struct ReleaseWorkflowInputs<'a> {
    pub platform: Platform,
    pub runner: &'a str,
    pub preinstalled_nix: bool,
    pub publish_enforcement: ReleasePublisherEnforcement,
    pub artifacts: &'a ArtifactsConfig,
    pub smoke_command: Option<&'a str>,
    pub release: Option<&'a ResolvedReleaseTarget>,
    pub attic: Option<&'a AtticConfig>,
    pub aur: Option<&'a ResolvedAur>,
    pub copr: Option<&'a ResolvedCopr>,
    pub apt: Option<&'a ResolvedApt>,
    pub homebrew: Option<&'a ResolvedHomebrew>,
    pub scoop: Option<&'a ResolvedScoop>,
    pub chocolatey: Option<&'a ResolvedChocolatey>,
    pub windows_signing: Option<&'a WindowsSigningConfig>,
    pub flatpak: Option<&'a FlatpakConfig>,
    pub winget: Option<&'a WingetConfig>,
    pub announce: Option<&'a AnnounceConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ReleaseCredential {
    pub name: String,
    pub kind: ReleaseCredentialKind,
    pub scope: ReleaseCredentialScope,
    pub channel: String,
    pub required: bool,
    pub context: ReleaseCredentialContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseCredentialKind {
    Secret,
    Variable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseCredentialScope {
    User,
    Repo,
    Org,
    Runner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseCredentialContext {
    Secrets,
    Vars,
    Env,
}

impl ReleaseCredential {
    fn secret(
        name: impl Into<String>,
        scope: ReleaseCredentialScope,
        channel: impl Into<String>,
        required: bool,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: ReleaseCredentialKind::Secret,
            scope,
            channel: channel.into(),
            required,
            context: ReleaseCredentialContext::Secrets,
            description: Some(description.into()),
        }
    }

    fn variable(
        name: impl Into<String>,
        scope: ReleaseCredentialScope,
        channel: impl Into<String>,
        required: bool,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: ReleaseCredentialKind::Variable,
            scope,
            channel: channel.into(),
            required,
            context: ReleaseCredentialContext::Vars,
            description: Some(description.into()),
        }
    }

    fn runner_env(
        name: impl Into<String>,
        channel: impl Into<String>,
        required: bool,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: ReleaseCredentialKind::Secret,
            scope: ReleaseCredentialScope::Runner,
            channel: channel.into(),
            required,
            context: ReleaseCredentialContext::Env,
            description: Some(description.into()),
        }
    }
}

pub fn credential_contract(inputs: &ReleaseWorkflowInputs<'_>) -> Vec<ReleaseCredential> {
    let mut credentials = Vec::new();
    if let Some(release) = inputs.release {
        credentials.push(ReleaseCredential::secret(
            &release.token_secret,
            ReleaseCredentialScope::User,
            match release.provider {
                ReleaseProvider::Forgejo => "codeberg",
                ReleaseProvider::Github => "github",
            },
            true,
            "Hosted release API token for release creation and asset upload.",
        ));
    }
    if inputs.artifacts.sign {
        credentials.push(ReleaseCredential::secret(
            "MINISIGN_SECRET_KEY",
            ReleaseCredentialScope::Repo,
            "minisign",
            true,
            "minisign secret key for signing SHA256SUMS.txt.",
        ));
        credentials.push(ReleaseCredential::secret(
            "MINISIGN_PASSWORD",
            ReleaseCredentialScope::Repo,
            "minisign",
            true,
            "minisign password for the release signing key.",
        ));
        credentials.push(ReleaseCredential::secret(
            "COSIGN_PRIVATE_KEY",
            ReleaseCredentialScope::Repo,
            "cosign",
            false,
            "Optional fallback cosign private key when keyless Sigstore OIDC is unavailable.",
        ));
        credentials.push(ReleaseCredential::secret(
            "COSIGN_PASSWORD",
            ReleaseCredentialScope::Repo,
            "cosign",
            false,
            "Optional password for COSIGN_PRIVATE_KEY.",
        ));
    }
    if let Some(apt) = inputs.apt {
        let apt_gpg_fingerprint_variable = apt_fingerprint_variable(apt);
        let apt_gpg_public_key_variable = apt_public_key_variable(apt);
        credentials.push(ReleaseCredential::secret(
            &apt.gpg_key_secret,
            ReleaseCredentialScope::Repo,
            "apt",
            true,
            "Armored GPG private key for the apt repository.",
        ));
        credentials.push(ReleaseCredential::variable(
            &apt.gpg_key_id_secret,
            ReleaseCredentialScope::Repo,
            "apt",
            true,
            "Short key id for the apt repository signing key.",
        ));
        credentials.push(ReleaseCredential::variable(
            apt_gpg_fingerprint_variable,
            ReleaseCredentialScope::Repo,
            "apt",
            true,
            "Fingerprint for the apt repository signing key.",
        ));
        credentials.push(ReleaseCredential::variable(
            apt_gpg_public_key_variable,
            ReleaseCredentialScope::Repo,
            "apt",
            true,
            "Armored public key published with the apt repository.",
        ));
        credentials.push(ReleaseCredential::secret(
            &apt.gpg_passphrase_secret,
            ReleaseCredentialScope::Repo,
            "apt",
            false,
            "Optional passphrase for the apt repository signing key.",
        ));
        credentials.push(ReleaseCredential::secret(
            &apt.ssh_key_secret,
            ReleaseCredentialScope::Repo,
            "apt",
            true,
            "Deploy key that pushes the generated apt repository.",
        ));
    }
    if let Some(aur) = inputs.aur {
        credentials.push(ReleaseCredential::secret(
            &aur.ssh_key_secret,
            ReleaseCredentialScope::User,
            "aur",
            true,
            "SSH private key for aur.archlinux.org pushes.",
        ));
    }
    if let Some(copr) = inputs.copr {
        credentials.push(ReleaseCredential::secret(
            &copr.login_secret,
            ReleaseCredentialScope::User,
            "copr",
            true,
            "COPR API login.",
        ));
        credentials.push(ReleaseCredential::variable(
            &copr.username_secret,
            ReleaseCredentialScope::User,
            "copr",
            true,
            "COPR username.",
        ));
        credentials.push(ReleaseCredential::secret(
            &copr.token_secret,
            ReleaseCredentialScope::User,
            "copr",
            true,
            "COPR API token.",
        ));
    }
    if let Some(homebrew) = inputs.homebrew {
        credentials.push(ReleaseCredential::secret(
            &homebrew.tap_token_secret,
            ReleaseCredentialScope::User,
            "homebrew",
            false,
            "Token for pushing the Homebrew tap.",
        ));
    }
    if let Some(scoop) = inputs.scoop {
        credentials.push(ReleaseCredential::secret(
            &scoop.bucket_token_secret,
            ReleaseCredentialScope::User,
            "scoop",
            false,
            "Token for pushing the Scoop bucket.",
        ));
    }
    if let Some(chocolatey) = inputs.chocolatey {
        if chocolatey.api_key_from_runner {
            credentials.push(ReleaseCredential::runner_env(
                &chocolatey.api_key_env,
                "chocolatey",
                true,
                "Runner-provided Chocolatey API key.",
            ));
        } else {
            credentials.push(ReleaseCredential::secret(
                &chocolatey.api_key_secret,
                ReleaseCredentialScope::User,
                "chocolatey",
                true,
                "Chocolatey push API key.",
            ));
        }
    }
    if let Some(flatpak) = inputs.flatpak {
        credentials.push(ReleaseCredential::secret(
            &flatpak.token_secret,
            ReleaseCredentialScope::User,
            "flatpak",
            false,
            "GitHub token for opening Flathub update PRs.",
        ));
    }
    if let Some(winget) = inputs.winget {
        credentials.push(ReleaseCredential::secret(
            &winget.token_secret,
            ReleaseCredentialScope::User,
            "winget",
            false,
            "GitHub token for submitting winget manifests.",
        ));
    }
    if let Some(windows) = inputs.windows_signing {
        credentials.push(ReleaseCredential::secret(
            &windows.pfx_secret,
            ReleaseCredentialScope::Repo,
            "windows-signing",
            false,
            "Optional Authenticode PKCS#12 certificate.",
        ));
        credentials.push(ReleaseCredential::secret(
            &windows.pass_secret,
            ReleaseCredentialScope::Repo,
            "windows-signing",
            false,
            "Optional Authenticode certificate passphrase.",
        ));
        credentials.push(ReleaseCredential::secret(
            &windows.subject_secret,
            ReleaseCredentialScope::Repo,
            "windows-signing",
            false,
            "Optional Authenticode signing subject.",
        ));
    }
    if let Some(announce) = inputs.announce {
        for (name, description) in [
            (
                &announce.mastodon_token_secret,
                "Mastodon token for configured release announcements.",
            ),
            (
                &announce.mastodon_base_url_secret,
                "Mastodon base URL for configured release announcements.",
            ),
            (
                &announce.matrix_token_secret,
                "Matrix token for configured release announcements.",
            ),
            (
                &announce.matrix_homeserver_secret,
                "Matrix homeserver for configured release announcements.",
            ),
            (
                &announce.matrix_room_secret,
                "Matrix room for configured release announcements.",
            ),
        ] {
            credentials.push(ReleaseCredential::secret(
                name,
                ReleaseCredentialScope::User,
                "announce",
                false,
                description,
            ));
        }
    }
    if let Some(attic) = inputs.attic {
        credentials.push(ReleaseCredential::runner_env(
            format!("{}/{}", attic.token_dir_env, attic.token_name),
            "attic",
            false,
            "Runner-provided Attic token file.",
        ));
    }
    credentials
}

fn apt_fingerprint_variable(apt: &ResolvedApt) -> String {
    apt.gpg_key_id_secret
        .replace("KEY_ID", "FINGERPRINT")
        .replace("key_id", "fingerprint")
}

fn apt_public_key_variable(apt: &ResolvedApt) -> String {
    apt.gpg_key_id_secret
        .replace("KEY_ID", "PUBLIC_KEY")
        .replace("key_id", "public_key")
}
/// The semver-with-optional-prerelease tag pattern shared by validation and gating.
const TAG_REGEX: &str = r"^[0-9]+\.[0-9]+\.[0-9]+(-(rc|beta|alpha)\.[0-9]+)?$";
const PRERELEASE_REGEX: &str = r"-(rc|beta|alpha)\.[0-9]+$";

/// Resolve `VERSION` from whichever forge ref-name env var is set.
const VERSION_FROM_REF: &str = r#"VERSION="${GITHUB_REF_NAME:-${FORGE_REF_NAME:-${CODEBERG_REF_NAME:-}}}"
          if [ -z "$VERSION" ]; then ref="${GITHUB_REF:-${FORGE_REF:-${CODEBERG_REF:-}}}"; VERSION="${ref#refs/tags/}"; fi"#;

pub fn render(inputs: &ReleaseWorkflowInputs<'_>) -> String {
    let mut w = String::new();
    let activated_remote =
        inputs.publish_enforcement == ReleasePublisherEnforcement::ActivatedRemote;
    w.push_str("---\n# yamllint disable rule:line-length\n");
    w.push_str(super::ci::GENERATED_WORKFLOW_MARKER);
    w.push('\n');
    push_secrets_header(&mut w, inputs);
    w.push_str("name: release\n\n");

    w.push_str("'on':\n");
    w.push_str("  push:\n    tags:\n      - '[0-9]*'\n");
    // Codeberg's current Gitea 1.22-derived Actions service rejects nested
    // workflow_dispatch input mappings.  Release publication is tag-driven;
    // an operator can dispatch this workflow against the tag ref directly.
    w.push_str("  workflow_dispatch:\n\n");

    w.push_str("concurrency:\n");
    w.push_str("  group: ${{ github.workflow }}-${{ github.ref }}\n");
    w.push_str("  cancel-in-progress: false\n\n");

    w.push_str("jobs:\n  release:\n");
    writeln!(w, "    runs-on: {}", inputs.runner).expect("write");
    match inputs.platform {
        Platform::Github => {
            w.push_str("    permissions:\n      contents: ");
            w.push_str(if inputs.release.is_some() {
                "write\n"
            } else {
                "read\n"
            });
            w.push_str("      id-token: write\n");
        }
        Platform::Forgejo => w.push_str("    enable-openid-connect: true\n"),
    }
    if inputs.preinstalled_nix {
        push_preinstalled_nix_env(&mut w, inputs.artifacts);
    }
    w.push_str("    steps:\n");

    push_checkout(&mut w, inputs.platform);
    if !inputs.preinstalled_nix {
        push_install_nix(&mut w, inputs.platform, inputs.artifacts);
    }
    push_validate_tag(&mut w, inputs.artifacts);
    push_release_credentials_preflight(&mut w, inputs);
    if let Some(copr) = inputs.copr {
        push_rewrite_spec(&mut w, copr);
    }
    if let Some(command) = &inputs.artifacts.supply_chain_command {
        push_supply_chain(&mut w, command);
    }
    if let Some(copr) = inputs.copr {
        push_build_srpm(&mut w, copr);
    }
    if !inputs.artifacts.sbom_commands.is_empty() {
        push_sbom(&mut w, &inputs.artifacts.sbom_commands);
    }
    push_build_artifacts(&mut w, inputs.artifacts);
    if let Some(apt) = inputs.apt {
        push_build_debs(&mut w, apt);
    }
    if let Some(windows) = inputs.windows_signing {
        push_windows_signing(&mut w, windows);
    }
    push_checksums(&mut w, inputs.artifacts);
    if inputs.artifacts.sign {
        push_sign(&mut w, inputs.artifacts, inputs.release, inputs.platform);
    }
    if let Some(command) = inputs.smoke_command {
        push_smoke(&mut w, command);
    }
    if let Some(release) = inputs.release {
        push_hosted_release(&mut w, release, inputs.artifacts);
    }
    if let Some(attic) = inputs.attic {
        push_attic(&mut w, attic);
    }
    if activated_remote {
        push_publisher_state_probe(&mut w, inputs);
    }
    if let Some(apt) = inputs.apt {
        push_publish_apt(&mut w, apt, activated_remote);
    }
    if let Some(announce) = inputs.announce {
        push_announce(&mut w, announce, inputs.release, inputs.platform);
    }
    if let Some(aur) = inputs.aur {
        push_publish_aur(&mut w, aur, activated_remote);
    }
    if let Some(flatpak) = inputs.flatpak {
        push_publish_flathub(&mut w, flatpak, activated_remote);
    }
    if let Some(winget) = inputs.winget {
        push_publish_winget(&mut w, winget, activated_remote, inputs.platform);
    }
    if let Some(homebrew) = inputs.homebrew {
        push_publish_homebrew(&mut w, homebrew, activated_remote, inputs.platform);
    }
    if let Some(scoop) = inputs.scoop {
        push_publish_scoop(&mut w, scoop, activated_remote);
    }
    if let Some(copr) = inputs.copr {
        push_publish_copr(&mut w, copr, activated_remote);
    }
    if let Some(chocolatey) = inputs.chocolatey {
        push_publish_chocolatey(&mut w, chocolatey, activated_remote);
    }

    w
}

fn push_secrets_header(w: &mut String, inputs: &ReleaseWorkflowInputs<'_>) {
    w.push_str("# Required secrets and variables:\n");
    if let Some(release) = inputs.release {
        writeln!(
            w,
            "# - {}: hosted release API token for release creation and asset upload.",
            release.token_secret
        )
        .expect("write");
    }
    if inputs.artifacts.sign {
        w.push_str(
            "# - MINISIGN_SECRET_KEY, MINISIGN_PASSWORD: minisign signing of SHA256SUMS.txt.\n",
        );
        w.push_str("# - COSIGN_PRIVATE_KEY, COSIGN_PASSWORD: optional fallback when keyless Sigstore OIDC is unavailable.\n");
    }
    if let Some(apt) = inputs.apt {
        writeln!(
            w,
            "# - {}: armored GPG private key for the apt repository.",
            apt.gpg_key_secret
        )
        .expect("write");
        writeln!(
            w,
            "# - vars.{}: fingerprint for the apt repository signing key.",
            apt.gpg_key_id_secret
        )
        .expect("write");
        writeln!(
            w,
            "# - {}: optional passphrase for the apt signing key.",
            apt.gpg_passphrase_secret
        )
        .expect("write");
        writeln!(
            w,
            "# - {}: ed25519 deploy key that pushes the apt tree.",
            apt.ssh_key_secret
        )
        .expect("write");
    }
    if let Some(aur) = inputs.aur {
        writeln!(
            w,
            "# - {}: Ed25519 private key for aur.archlinux.org pushes.",
            aur.ssh_key_secret
        )
        .expect("write");
    }
    if let Some(copr) = inputs.copr {
        writeln!(
            w,
            "# - {}, {}, {}: COPR upload credentials; {} is an Actions variable.",
            copr.login_secret, copr.username_secret, copr.token_secret, copr.username_secret
        )
        .expect("write");
    }
    if let Some(homebrew) = inputs.homebrew {
        if homebrew.tap_token_secret != "homebrew_tap_token" {
            writeln!(
                w,
                "# - {}: optional Homebrew tap push token.",
                homebrew.tap_token_secret
            )
            .expect("write");
        }
    }
    if let Some(scoop) = inputs.scoop {
        if scoop.bucket_token_secret != "SCOOP_BUCKET_TOKEN" {
            writeln!(
                w,
                "# - {}: optional Scoop bucket push token.",
                scoop.bucket_token_secret
            )
            .expect("write");
        }
    }
    if let Some(windows) = inputs.windows_signing {
        writeln!(
            w,
            "# - {}, {}: optional Authenticode PKCS#12 cert + passphrase.",
            windows.pfx_secret, windows.pass_secret
        )
        .expect("write");
    }
    if let Some(chocolatey) = inputs.chocolatey {
        if chocolatey.api_key_from_runner {
            writeln!(
                w,
                "# - runner-provided env {}: Chocolatey push API key.",
                chocolatey.api_key_env
            )
            .expect("write");
        } else {
            writeln!(
                w,
                "# - {}: optional Chocolatey push API key (env {}).",
                chocolatey.api_key_secret, chocolatey.api_key_env
            )
            .expect("write");
        }
    }
    if let Some(flatpak) = inputs.flatpak {
        writeln!(
            w,
            "# - {}: optional GitHub PAT for {} Flathub PRs.",
            flatpak.token_secret, flatpak.repo
        )
        .expect("write");
    }
    if let Some(winget) = inputs.winget {
        writeln!(
            w,
            "# - {}: optional GitHub PAT for winget-pkgs PRs.",
            winget.token_secret
        )
        .expect("write");
    }
    if let Some(announce) = inputs.announce {
        writeln!(
            w,
            "# - {}, {}, {}, {}, {}: required when release announcements are configured.",
            announce.mastodon_token_secret,
            announce.mastodon_base_url_secret,
            announce.matrix_token_secret,
            announce.matrix_homeserver_secret,
            announce.matrix_room_secret
        )
        .expect("write");
    }
    if let Some(attic) = inputs.attic {
        writeln!(
            w,
            "# Runner credential: ${}/{}: Attic token file.",
            attic.token_dir_env, attic.token_name
        )
        .expect("write");
    }
    w.push('\n');
}

fn push_checkout(w: &mut String, platform: Platform) {
    w.push_str("      - uses: ");
    if platform == Platform::Forgejo {
        w.push_str(&forgejo_action_ref("checkout", "v4.3.1"));
    } else {
        w.push_str(&github_action_ref("actions/checkout", "v4.3.1"));
    }
    w.push('\n');
    w.push_str("        with:\n          fetch-depth: 0\n");
}

fn push_install_nix(w: &mut String, platform: Platform, artifacts: &ArtifactsConfig) {
    w.push_str("      - uses: ");
    if platform == Platform::Forgejo {
        w.push_str(&immutable_action_ref(
            "https://github.com/cachix/install-nix-action",
            "v27",
        ));
    } else {
        w.push_str(&github_action_ref("cachix/install-nix-action", "v27"));
    }
    w.push('\n');
    w.push_str("        with:\n          extra_nix_config: |\n");
    w.push_str("            experimental-features = nix-command flakes\n");
    if !artifacts.substituters.is_empty() {
        writeln!(
            w,
            "            substituters = {}",
            artifacts.substituters.join(" ")
        )
        .expect("write");
    }
    if !artifacts.trusted_public_keys.is_empty() {
        writeln!(
            w,
            "            trusted-public-keys = {}",
            artifacts.trusted_public_keys.join(" ")
        )
        .expect("write");
    }
    w.push('\n');
}

fn push_preinstalled_nix_env(w: &mut String, artifacts: &ArtifactsConfig) {
    w.push_str("    env:\n");
    w.push_str("      NIX_CONFIG: |\n");
    w.push_str("        experimental-features = nix-command flakes\n");
    w.push_str("        accept-flake-config = true\n");
    if !artifacts.substituters.is_empty() {
        writeln!(
            w,
            "        substituters = {}",
            artifacts.substituters.join(" ")
        )
        .expect("write");
    }
    if !artifacts.trusted_public_keys.is_empty() {
        writeln!(
            w,
            "        trusted-public-keys = {}",
            artifacts.trusted_public_keys.join(" ")
        )
        .expect("write");
    }
    w.push_str("      XDG_CACHE_HOME: \"/tmp/.cache\"\n");
}

fn push_validate_tag(w: &mut String, artifacts: &ArtifactsConfig) {
    w.push_str("      - name: Validate tag\n        run: |\n          set -euo pipefail\n");
    writeln!(w, "          {VERSION_FROM_REF}").expect("write");
    writeln!(w, "          echo \"$VERSION\" | grep -Eq '{TAG_REGEX}'").expect("write");
    w.push_str(
        "          git fetch --force --tags origin \"refs/tags/${VERSION}:refs/tags/${VERSION}\"\n",
    );
    w.push_str("          tag_worktree=\"$(mktemp -d)\"\n");
    w.push_str("          rmdir \"$tag_worktree\"\n");
    w.push_str("          cleanup_validation() {\n");
    w.push_str("            if [ -n \"${tag_worktree:-}\" ]; then git worktree remove --force \"$tag_worktree\" >/dev/null 2>&1 || rm -rf \"$tag_worktree\"; fi\n");
    w.push_str("            if [ -n \"${GNUPGHOME:-}\" ]; then rm -rf \"$GNUPGHOME\"; fi\n");
    w.push_str("          }\n");
    w.push_str("          trap cleanup_validation EXIT\n");
    w.push_str("          git worktree add --detach \"$tag_worktree\" \"$VERSION\"\n");
    if let Some(attr) = &artifacts.version_attr {
        writeln!(
            w,
            "          test \"$(nix eval --raw \"$tag_worktree#{attr}.version\")\" = \"$VERSION\""
        )
        .expect("write");
    }
    w.push_str("          test -s \"$tag_worktree/keys/maintainers.gpg\"\n");
    w.push_str("          if ! grep -q \"^## \\[$VERSION\\] - [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]\" \"$tag_worktree/CHANGELOG.md\"; then\n");
    w.push_str("            echo \"CHANGELOG.md missing section for $VERSION\" >&2\n            exit 1\n          fi\n\n");
    w.push_str("          IS_PRERELEASE=false\n");
    writeln!(w, "          if printf '%s\\n' \"$VERSION\" | grep -Eq -- '{PRERELEASE_REGEX}'; then IS_PRERELEASE=true; fi").expect("write");
    w.push_str(
        "\n          GNUPGHOME=\"$(mktemp -d)\"; export GNUPGHOME; chmod 700 \"$GNUPGHOME\"\n",
    );
    w.push_str("          gpg --batch --import \"$tag_worktree/keys/maintainers.gpg\"\n");
    w.push_str("          git verify-tag \"$VERSION\"\n");
    w.push_str("          validated_sha=\"$(git -C \"$tag_worktree\" rev-parse HEAD)\"\n");
    w.push_str("          git checkout --detach \"$validated_sha\"\n");
    w.push_str("          { printf 'VERSION=%s\\n' \"$VERSION\"; printf 'IS_PRERELEASE=%s\\n' \"$IS_PRERELEASE\"; } > release-env\n");
}

fn push_release_credentials_preflight(w: &mut String, inputs: &ReleaseWorkflowInputs<'_>) {
    let credentials = credential_contract(inputs)
        .into_iter()
        .filter(|credential| {
            credential.required
                && (inputs.publish_enforcement == ReleasePublisherEnforcement::Declared
                    || !downstream_publisher_credential(credential))
        })
        .collect::<Vec<_>>();
    if credentials.is_empty() {
        return;
    }

    w.push_str("      - name: Check release credentials\n");
    w.push_str("        env:\n");
    for credential in &credentials {
        writeln!(
            w,
            "          {}: {}",
            credential_env_name(credential),
            credential_expression(credential)
        )
        .expect("write");
    }
    w.push_str("        run: |\n          set -euo pipefail\n          . ./release-env\n");
    w.push_str("          missing=''\n");
    w.push_str("          require_credential() {\n");
    w.push_str("            scope=\"$1\"; name=\"$2\"; value=\"$3\"\n");
    w.push_str(
        "            if [ -z \"$value\" ]; then missing=\"$(printf '%s\\n  - %s %s' \"$missing\" \"$scope\" \"$name\")\"; fi\n",
    );
    w.push_str("          }\n");
    for credential in &credentials {
        let indent = if stable_only_preflight(credential) {
            w.push_str("          if [ \"${IS_PRERELEASE}\" != \"true\" ]; then\n");
            "            "
        } else {
            "          "
        };
        writeln!(
            w,
            "{indent}require_credential '{}' '{}' \"${{{}:-}}\"",
            credential_scope_label(credential),
            credential.name,
            credential_env_name(credential)
        )
        .expect("write");
        if stable_only_preflight(credential) {
            w.push_str("          fi\n");
        }
    }
    w.push_str("          if [ -n \"$missing\" ]; then\n");
    w.push_str("            printf 'Missing release credentials:%s\\n' \"$missing\" >&2\n");
    w.push_str("            exit 1\n");
    w.push_str("          fi\n");
    if inputs.artifacts.sign {
        writeln!(w, "          test -s {}", inputs.artifacts.minisign_pub).expect("write");
        w.push_str("          umask 077\n");
        w.push_str("          minisign_key=\"$(mktemp)\"; minisign_probe=\"$(mktemp)\"; minisign_sig=\"${minisign_probe}.minisig\"\n");
        w.push_str(
            "          trap 'rm -f \"$minisign_key\" \"$minisign_probe\" \"$minisign_sig\"' EXIT\n",
        );
        w.push_str("          printf '%s' \"$MINISIGN_SECRET_KEY\" > \"$minisign_key\"\n");
        w.push_str("          printf 'simit release credentials probe\\n' > \"$minisign_probe\"\n");
        w.push_str("          printf '%s\\n' \"$MINISIGN_PASSWORD\" | \\\n");
        w.push_str("            nix shell nixpkgs#minisign -c minisign -S -s \"$minisign_key\" -m \"$minisign_probe\" -x \"$minisign_sig\"\n");
        writeln!(
            w,
            "          nix shell nixpkgs#minisign -c minisign -V -m \"$minisign_probe\" -x \"$minisign_sig\" -p {}",
            inputs.artifacts.minisign_pub
        )
        .expect("write");
    }
}

fn credential_env_name(credential: &ReleaseCredential) -> String {
    let mut out = credential
        .name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_owned()
}

fn credential_expression(credential: &ReleaseCredential) -> String {
    match credential.context {
        ReleaseCredentialContext::Secrets => {
            format!("${{{{ secrets.{} }}}}", credential.name)
        }
        ReleaseCredentialContext::Vars => format!("${{{{ vars.{} }}}}", credential.name),
        ReleaseCredentialContext::Env => format!("${{{{ env.{} }}}}", credential.name),
    }
}

fn credential_scope_label(credential: &ReleaseCredential) -> &'static str {
    match (credential.scope, credential.kind) {
        (ReleaseCredentialScope::User, ReleaseCredentialKind::Secret) => "global/user secret",
        (ReleaseCredentialScope::User, ReleaseCredentialKind::Variable) => "global/user variable",
        (ReleaseCredentialScope::Repo, ReleaseCredentialKind::Secret) => "repo secret",
        (ReleaseCredentialScope::Repo, ReleaseCredentialKind::Variable) => "repo variable",
        (ReleaseCredentialScope::Org, ReleaseCredentialKind::Secret) => "org secret",
        (ReleaseCredentialScope::Org, ReleaseCredentialKind::Variable) => "org variable",
        (ReleaseCredentialScope::Runner, _) => "runner credential",
    }
}

fn stable_only_preflight(credential: &ReleaseCredential) -> bool {
    matches!(
        credential.channel.as_str(),
        "apt" | "aur" | "homebrew" | "scoop" | "chocolatey" | "flatpak" | "winget"
    )
}

fn downstream_publisher_credential(credential: &ReleaseCredential) -> bool {
    matches!(
        credential.channel.as_str(),
        "apt" | "aur" | "copr" | "homebrew" | "scoop" | "chocolatey" | "flatpak" | "winget"
    )
}

fn push_rewrite_spec(w: &mut String, copr: &ResolvedCopr) {
    writeln!(w, "      - name: Rewrite {} Version", copr.spec_path).expect("write");
    w.push_str("        run: |\n          . ./release-env\n");
    writeln!(
        w,
        "          sed -i \"0,/^Version:.*$/s//Version:        ${{VERSION}}/\" {}",
        copr.spec_path
    )
    .expect("write");
    writeln!(w, "          grep '^Version:' {}", copr.spec_path).expect("write");
}

fn push_supply_chain(w: &mut String, command: &str) {
    w.push_str("      - name: Check supply-chain policy\n        run: |\n");
    for line in command.lines() {
        writeln!(w, "          {line}").expect("write");
    }
}

fn push_build_srpm(w: &mut String, copr: &ResolvedCopr) {
    w.push_str("      - name: Build SRPM for COPR\n        run: |\n          set -euo pipefail\n          . ./release-env\n");
    writeln!(
        w,
        "          git archive --format=tar.gz --prefix={repo}/ -o \"{repo}-${{VERSION}}.tar.gz\" HEAD",
        repo = copr.repo
    )
    .expect("write");
    w.push_str("          nix shell nixpkgs#rpm nixpkgs#cargo nixpkgs#rustc -c bash <<'SCRIPT'\n");
    w.push_str("          set -euo pipefail\n");
    writeln!(w, "          tar xf {repo}-*.tar.gz", repo = copr.repo).expect("write");
    writeln!(
        w,
        "          ( cd {repo} && cargo vendor vendor > ../cargo-vendor-config.toml )",
        repo = copr.repo
    )
    .expect("write");
    writeln!(
        w,
        "          tar czf vendor.tar.gz -C {repo} vendor",
        repo = copr.repo
    )
    .expect("write");
    w.push_str("          rm -rf srpms && mkdir -p srpms\n");
    writeln!(
        w,
        "          rpmbuild -bs {spec} --define \"_sourcedir $(pwd)\" --define \"_srcrpmdir $(pwd)/srpms\"",
        spec = copr.spec_path
    )
    .expect("write");
    w.push_str("          set -- srpms/*.src.rpm; test \"$#\" -eq 1; ls -l \"$1\"\n");
    w.push_str("          SCRIPT\n");
}

fn push_build_artifacts(w: &mut String, artifacts: &ArtifactsConfig) {
    let bundle_attrs = artifacts.effective_nix_bundle_attrs();
    w.push_str(
        "      - name: Build release artifacts\n        run: |\n          set -euo pipefail\n",
    );
    w.push_str("          . ./release-env\n          export VERSION IS_PRERELEASE\n          mkdir -p release\n");
    if !bundle_attrs.is_empty() {
        w.push_str("          mkdir -p target\n");
    }
    for (index, attr) in bundle_attrs.iter().enumerate() {
        let link = format!("target/simit-release-bundle-{index}");
        writeln!(
            w,
            "          nix build {} --out-link {}",
            shell_single_quote(&format!(".#{}", attr)),
            shell_single_quote(&link)
        )
        .expect("write");
        writeln!(w, "          test -d {}", shell_single_quote(&link)).expect("write");
        w.push_str("          while IFS= read -r file; do\n");
        w.push_str("            name=\"$(basename \"$file\")\"\n");
        w.push_str("            test ! -e \"release/$name\" || { echo \"release bundle asset collision: $name\" >&2; exit 1; }\n");
        w.push_str("            cp -L \"$file\" \"release/$name\"\n");
        w.push_str("          done < <(find -L ");
        w.push_str(&shell_single_quote(&link));
        w.push_str(" -mindepth 1 -maxdepth 1 -type f -print | LC_ALL=C sort)\n");
    }
    if !bundle_attrs.is_empty() {
        w.push_str("          shopt -s nullglob\n");
        w.push_str("          manifests=(release/*-release-manifest.json)\n");
        w.push_str("          test \"${#manifests[@]}\" -eq 1 || { echo \"expected exactly one release manifest from Nix bundles\" >&2; exit 1; }\n");
        w.push_str("          jq -e --arg version \"$VERSION\" '(.schemaVersion == 2) and (.version == $version) and (.artifacts | length > 0)' \"${manifests[0]}\" >/dev/null\n");
    }
    if artifacts.build_commands.is_empty() {
        if bundle_attrs.is_empty() {
            w.push_str("          echo \"[release.artifacts] has no Nix bundles or build_commands\" >&2\n          exit 1\n");
        }
    } else {
        for line in &artifacts.build_commands {
            for sub in line.split('\n') {
                writeln!(w, "          {sub}").expect("write");
            }
        }
    }
}

fn push_build_debs(w: &mut String, _apt: &ResolvedApt) {
    w.push_str("      - name: Build Debian packages\n        run: |\n          set -euo pipefail\n          . ./release-env\n          export VERSION IS_PRERELEASE\n          mkdir -p release\n");
    w.push_str("          if ! nix develop -c bash scripts/build-deb.sh \"$VERSION\" release\n");
    w.push_str("          then\n");
    w.push_str("            echo \"::warning::Debian package build failed; continuing without .deb release assets and APT publish.\"\n");
    w.push_str("          fi\n");
}

fn push_checksums(w: &mut String, artifacts: &ArtifactsConfig) {
    w.push_str("      - name: Generate release checksums\n        run: |\n          set -euo pipefail\n          cd release\n");
    w.push_str("          shopt -s nullglob\n");
    w.push_str("          files=()\n");
    w.push_str("          add_matches() { local pattern=\"$1\" match; while IFS= read -r match; do files+=(\"$match\"); done < <(compgen -G \"$pattern\" || true); }\n");
    if artifacts.checksum_globs.is_empty() {
        w.push_str("          add_matches '*'\n");
    } else {
        for glob in &artifacts.checksum_globs {
            writeln!(w, "          add_matches {}", shell_single_quote(glob)).expect("write");
        }
    }
    w.push_str("          : > SHA256SUMS.txt\n");
    w.push_str("          count=0\n");
    w.push_str("          while IFS= read -r file; do\n");
    w.push_str("            [ -f \"$file\" ] || continue\n");
    w.push_str("            [ \"$file\" != SHA256SUMS.txt ] || continue\n");
    w.push_str("            sha256sum \"$file\" >> SHA256SUMS.txt\n");
    w.push_str("            count=$((count + 1))\n");
    w.push_str("          done < <(printf '%s\\n' \"${files[@]}\" | LC_ALL=C sort -u)\n");
    w.push_str("          test \"$count\" -gt 0\n");
    w.push_str("          test -s SHA256SUMS.txt\n");
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn push_sign(
    w: &mut String,
    artifacts: &ArtifactsConfig,
    release: Option<&ResolvedReleaseTarget>,
    platform: Platform,
) {
    w.push_str("      - name: Sign checksums and attest release artifacts\n");
    w.push_str("        env:\n");
    w.push_str("          MINISIGN_SECRET_KEY: ${{ secrets.MINISIGN_SECRET_KEY }}\n");
    w.push_str("          MINISIGN_PASSWORD: ${{ secrets.MINISIGN_PASSWORD }}\n");
    w.push_str("          COSIGN_PRIVATE_KEY: ${{ secrets.COSIGN_PRIVATE_KEY }}\n");
    w.push_str("          COSIGN_PASSWORD: ${{ secrets.COSIGN_PASSWORD }}\n");
    w.push_str("        run: |\n          set -euo pipefail\n          . ./release-env\n");
    writeln!(w, "          test -s {}", artifacts.minisign_pub).expect("write");
    w.push_str("          test -n \"${MINISIGN_SECRET_KEY:-}\"\n          test -n \"${MINISIGN_PASSWORD:-}\"\n");
    w.push_str("          umask 077\n          minisign_key=\"$(mktemp)\"; oidc_token=\"$(mktemp)\"; cosign_key=\"$(mktemp)\"; export oidc_token cosign_key\n");
    w.push_str("          trap 'rm -f \"$minisign_key\" \"$oidc_token\" \"$cosign_key\"' EXIT\n");
    w.push_str("          printf '%s' \"$MINISIGN_SECRET_KEY\" > \"$minisign_key\"\n");
    w.push_str("          printf '%s\\n' \"$MINISIGN_PASSWORD\" | \\\n");
    w.push_str("            nix shell nixpkgs#minisign -c minisign -S -s \"$minisign_key\" -m release/SHA256SUMS.txt -x release/SHA256SUMS.txt.minisig\n");
    writeln!(
        w,
        "          nix shell nixpkgs#minisign -c minisign -V -m release/SHA256SUMS.txt -x release/SHA256SUMS.txt.minisig -p {}\n",
        artifacts.minisign_pub
    )
    .expect("write");

    // Cosign keyless (Sigstore OIDC) signing + SLSA provenance attestation, with
    // a COSIGN_PRIVATE_KEY fallback — mirrors the hand-rolled release pipeline.
    let repo_url = release
        .map(|c| {
            let host = match c.provider {
                ReleaseProvider::Forgejo => "https://codeberg.org",
                ReleaseProvider::Github => "https://github.com",
            };
            format!("{host}/{}", c.repo)
        })
        .unwrap_or_else(|| format!("{}/unknown/unknown", platform.web_base_url()));
    w.push_str("          nix shell nixpkgs#cosign nixpkgs#curl nixpkgs#jq -c bash <<'SCRIPT'\n");
    w.push_str("          set -euo pipefail\n          . ./release-env\n");
    writeln!(w, "          repo_url=\"{repo_url}\"").expect("write");
    w.push_str("          git_sha=\"$(git rev-parse HEAD)\"\n");
    w.push_str("          workflow_sha=\"$(sha256sum ");
    w.push_str(platform.workflow_dir());
    w.push_str("/release.yml | awk '{print $1}')\"\n");
    w.push_str("          flake_lock_sha=\"missing\"; if [ -f flake.lock ]; then flake_lock_sha=\"$(sha256sum flake.lock | awk '{print $1}')\"; fi\n");
    w.push_str("          builder_id=\"");
    w.push('$');
    w.push_str("{repo_url}/src/tag/");
    w.push('$');
    w.push_str("{VERSION}/");
    w.push_str(platform.workflow_dir());
    w.push_str("/release.yml\"\n");
    w.push_str("          sign_blob_keyless() { file=\"$1\"; if [ -n \"${ACTIONS_ID_TOKEN_REQUEST_URL:-}\" ] && [ -n \"${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-}\" ]; then curl -fsSL -H \"Authorization: bearer ${ACTIONS_ID_TOKEN_REQUEST_TOKEN}\" \"${ACTIONS_ID_TOKEN_REQUEST_URL}&audience=sigstore\" > \"$oidc_token\"; cosign sign-blob --yes --identity-token \"$oidc_token\" --bundle \"${file}.cosign.bundle\" \"$file\"; else return 1; fi; }\n");
    w.push_str("          attest_blob_keyless() { file=\"$1\"; predicate=\"$2\"; if [ -s \"$oidc_token\" ]; then cosign attest-blob --yes --identity-token \"$oidc_token\" --predicate \"$predicate\" --type slsaprovenance1 --output-attestation \"${file}.intoto.jsonl\" --bundle \"${file}.intoto.bundle\" \"$file\"; else return 1; fi; }\n");
    w.push_str("          sign_blob_with_key() { file=\"$1\"; test -n \"${COSIGN_PRIVATE_KEY:-}\"; printf '%s' \"$COSIGN_PRIVATE_KEY\" > \"$cosign_key\"; cosign sign-blob --yes --key \"$cosign_key\" --bundle \"${file}.cosign.bundle\" \"$file\"; }\n");
    w.push_str("          attest_blob_with_key() { file=\"$1\"; predicate=\"$2\"; cosign attest-blob --yes --key \"$cosign_key\" --predicate \"$predicate\" --type slsaprovenance1 --output-attestation \"${file}.intoto.jsonl\" --bundle \"${file}.intoto.bundle\" \"$file\"; }\n");
    w.push_str("          shopt -s nullglob\n");
    w.push_str("          files=(release/*.tar.gz release/*.zip release/*.AppImage release/*.deb release/*.src.rpm release/*.exe)\n");
    w.push_str("          while IFS= read -r file; do\n");
    w.push_str("            [ -f \"$file\" ] || continue\n");
    w.push_str("            artifact_sha=\"$(sha256sum \"$file\" | awk '{print $1}')\"; predicate=\"$(mktemp)\"\n");
    w.push_str("            jq -n --arg builder_id \"$builder_id\" --arg git_sha \"$git_sha\" --arg workflow_sha \"$workflow_sha\" --arg flake_lock_sha \"$flake_lock_sha\" --arg repo_url \"$repo_url\" --arg ref \"refs/tags/${VERSION}\" --arg artifact \"$(basename \"$file\")\" --arg artifact_sha \"$artifact_sha\" '{buildDefinition:{buildType:\"https://simit.rs/release\",externalParameters:{repository:$repo_url,ref:$ref,artifact:$artifact,artifactDigest:{sha256:$artifact_sha}},internalParameters:{},resolvedDependencies:[{uri:($repo_url+\".git\"),digest:{gitCommit:$git_sha}},{uri:\"flake.lock\",digest:{sha256:$flake_lock_sha}},{uri:\"release workflow\",digest:{sha256:$workflow_sha}}]},runDetails:{builder:{id:$builder_id}}}' > \"$predicate\"\n");
    w.push_str("            if sign_blob_keyless \"$file\" && attest_blob_keyless \"$file\" \"$predicate\"; then echo \"signed+attested $file (keyless)\"; elif [ -n \"${COSIGN_PRIVATE_KEY:-}\" ]; then echo \"keyless failed for $file; using COSIGN_PRIVATE_KEY\"; sign_blob_with_key \"$file\"; attest_blob_with_key \"$file\" \"$predicate\"; else echo \"::warning::keyless Sigstore failed and COSIGN_PRIVATE_KEY unset; continuing without cosign signature or attestation for $file\" >&2; fi\n");
    w.push_str("            rm -f \"$predicate\"\n          done < <(printf '%s\\n' \"${files[@]}\" | LC_ALL=C sort -u)\n          SCRIPT\n");
}

fn push_smoke(w: &mut String, command: &str) {
    w.push_str("      - name: Run release smoke checks\n");
    w.push_str("        run: |\n          set -euo pipefail\n          . ./release-env\n          mkdir -p release\n");
    writeln!(w, "          {command} \"$VERSION\" release").expect("write");
}

fn push_attic(w: &mut String, attic: &AtticConfig) {
    w.push_str(
        "      - name: Push Nix closures to Attic\n        run: |\n          set -euo pipefail\n",
    );
    writeln!(
        w,
        "          attic_token_dir=\"${{{}:-}}\"",
        attic.token_dir_env
    )
    .expect("write");
    writeln!(
        w,
        "          attic_token=\"${{attic_token_dir}}/{}\"",
        attic.token_name
    )
    .expect("write");
    writeln!(
        w,
        "          if [ -z \"$attic_token_dir\" ] || [ ! -r \"$attic_token\" ]; then\n            echo \"::error::Attic token ${{{}:-<unset>}}/{} is required because Nix closure cache publishing is configured.\" >&2\n            exit 1\n          fi",
        attic.token_dir_env, attic.token_name
    )
    .expect("write");
    if !attic.result_links.is_empty() {
        w.push_str("          nix path-info -r \\\n");
        for link in &attic.result_links {
            let path = nix_path_info_arg(link);
            writeln!(w, "            {path} \\").expect("write");
        }
        w.push_str("            > attic-paths.txt\n");
    }
    w.push_str("          nix profile install nixpkgs#attic-client\n");
    writeln!(
        w,
        "          if ! attic login {cache} {url} \"$(cat \"$attic_token\")\"; then\n            echo \"::error::Attic login failed for configured Nix closure cache publishing.\" >&2\n            exit 1\n          fi",
        cache = attic.cache,
        url = attic.url
    )
    .expect("write");
    writeln!(
        w,
        "          if ! attic push {cache} $(cat attic-paths.txt); then\n            echo \"::error::Attic push failed for configured Nix closure cache publishing.\" >&2\n            exit 1\n          fi",
        cache = attic.cache
    )
    .expect("write");
}

fn push_hosted_release(
    w: &mut String,
    target: &ResolvedReleaseTarget,
    artifacts: &ArtifactsConfig,
) {
    match target.provider {
        ReleaseProvider::Forgejo => push_codeberg_release(w, target, artifacts),
        ReleaseProvider::Github => push_github_release(w, target, artifacts),
    }
}

fn push_github_release(
    w: &mut String,
    github: &ResolvedReleaseTarget,
    artifacts: &ArtifactsConfig,
) {
    w.push_str("      - name: Publish GitHub release\n        env:\n");
    w.push_str("          GITHUB_TOKEN: $");
    w.push_str("{{ secrets.");
    w.push_str(&github.token_secret);
    w.push_str(" }}\n");
    writeln!(w, "          GITHUB_API: {}", github.api_base).expect("write");
    writeln!(w, "          GITHUB_REPO: {}", github.repo).expect("write");
    w.push_str("        run: |\n          set -euo pipefail\n          test -n \"$GITHUB_TOKEN\"\n          . ./release-env\n");
    w.push_str("          nix shell nixpkgs#curl nixpkgs#jq -c bash <<'SCRIPT'\n");
    w.push_str("          set -euo pipefail\n          . ./release-env\n");
    if github.body_from_changelog {
        w.push_str("          awk -v version=\"$VERSION\" '\n");
        w.push_str("            $0 ~ \"^## \\\\[\" version \"\\\\] - [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]$\" { found = 1; print; next }\n");
        w.push_str("            found && /^## \\[/ { exit }\n");
        w.push_str("            found && /^\\[[^]]+\\]: / { exit }\n");
        w.push_str("            found { print }\n");
        w.push_str("            END { if (!found) exit 1 }\n");
        w.push_str("          ' CHANGELOG.md > release-notes.md || { echo \"CHANGELOG.md missing section for $VERSION\" >&2; exit 1; }\n");
    } else {
        w.push_str("          : > release-notes.md\n");
    }
    w.push_str("          release_payload=$(jq -n --arg tag \"$VERSION\" --arg name \"$VERSION\" --arg branch \"");
    w.push_str(&github.target_branch);
    w.push_str("\" --argjson prerelease \"$IS_PRERELEASE\" --rawfile body release-notes.md '{tag_name: $tag, target_commitish: $branch, name: $name, body: $body, draft: false, prerelease: $prerelease}')\n");
    w.push_str("          status=$(curl -sS -o release.json -w '%{http_code}' -H \"Authorization: Bearer $GITHUB_TOKEN\" -H 'Accept: application/vnd.github+json' -H 'X-GitHub-Api-Version: 2022-11-28' -d \"$release_payload\" \"$GITHUB_API/repos/$GITHUB_REPO/releases\")\n");
    w.push_str("          if [ \"$status\" = \"409\" ] || [ \"$status\" = \"422\" ]; then\n");
    w.push_str("            curl -sS --fail -H \"Authorization: Bearer $GITHUB_TOKEN\" -H 'Accept: application/vnd.github+json' \"$GITHUB_API/repos/$GITHUB_REPO/releases/tags/$VERSION\" > release.json\n");
    w.push_str("          elif [ \"$status\" -lt 200 ] || [ \"$status\" -ge 300 ]; then cat release.json; exit 1; fi\n");
    w.push_str(
        "          release_id=$(jq -r '.id' release.json); test \"$release_id\" != \"null\"\n",
    );
    w.push_str("          upload_url=$(jq -r '.upload_url' release.json | cut -d '{' -f 1); test -n \"$upload_url\"\n");
    w.push_str("          shopt -s nullglob\n          files=()\n");
    w.push_str("          add_matches() { local pattern=\"$1\" match; while IFS= read -r match; do files+=(\"$match\"); done < <(compgen -G \"$pattern\" || true); }\n");
    w.push_str("          add_matches 'release/SHA256SUMS.txt'\n          add_matches 'release/SHA256SUMS.txt.minisig'\n");
    if artifacts.checksum_globs.is_empty() {
        for glob in [
            "*.tar.gz",
            "*.zip",
            "*.AppImage",
            "*.deb",
            "*.src.rpm",
            "*.exe",
        ] {
            writeln!(
                w,
                "          add_matches {}",
                shell_single_quote(&format!("release/{glob}"))
            )
            .expect("write");
        }
    } else {
        for glob in &artifacts.checksum_globs {
            writeln!(
                w,
                "          add_matches {}",
                shell_single_quote(&format!("release/{glob}"))
            )
            .expect("write");
        }
    }
    w.push_str("          while IFS= read -r file; do\n");
    w.push_str(
        "            [ -f \"$file\" ] || continue\n            name=$(basename \"$file\")\n",
    );
    w.push_str("            asset_id=$(curl -sS --fail -H \"Authorization: Bearer $GITHUB_TOKEN\" -H 'Accept: application/vnd.github+json' \"$GITHUB_API/repos/$GITHUB_REPO/releases/$release_id/assets\" | jq -r --arg name \"$name\" '.[] | select(.name == $name) | .id' | head -n 1)\n");
    w.push_str("            if [ -n \"$asset_id\" ]; then curl -sS --fail -X DELETE -H \"Authorization: Bearer $GITHUB_TOKEN\" -H 'Accept: application/vnd.github+json' \"$GITHUB_API/repos/$GITHUB_REPO/releases/$release_id/assets/$asset_id\"; fi\n");
    w.push_str("            curl -sS --fail -H \"Authorization: Bearer $GITHUB_TOKEN\" -H 'Accept: application/vnd.github+json' -H 'Content-Type: application/octet-stream' --data-binary \"@$file\" \"$upload_url?name=$name\" > /dev/null\n");
    w.push_str("          done < <(printf '%s\\n' \"");
    w.push('$');
    w.push_str("{files[@]}\" | LC_ALL=C sort -u)\n          SCRIPT\n");
}

fn push_codeberg_release(
    w: &mut String,
    codeberg: &ResolvedReleaseTarget,
    artifacts: &ArtifactsConfig,
) {
    w.push_str("      - name: Publish Codeberg release\n        env:\n");
    writeln!(
        w,
        "          CODEBERG_TOKEN: ${{{{ secrets.{} }}}}",
        codeberg.token_secret
    )
    .expect("write");
    writeln!(w, "          CODEBERG_API: {}", codeberg.api_base).expect("write");
    writeln!(w, "          CODEBERG_REPO: {}", codeberg.repo).expect("write");
    w.push_str("        run: |\n          set -euo pipefail\n          test -n \"$CODEBERG_TOKEN\"\n          . ./release-env\n");
    w.push_str("          nix shell nixpkgs#curl nixpkgs#jq -c bash <<'SCRIPT'\n");
    w.push_str("          set -euo pipefail\n          . ./release-env\n");
    if codeberg.body_from_changelog {
        w.push_str("          awk -v version=\"$VERSION\" '\n");
        w.push_str("            $0 ~ \"^## \\\\[\" version \"\\\\] - [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]$\" { found = 1; print; next }\n");
        w.push_str("            found && /^## \\[/ { exit }\n");
        w.push_str("            found && /^\\[[^]]+\\]: / { exit }\n");
        w.push_str("            found { print }\n");
        w.push_str("            END { if (!found) exit 1 }\n");
        w.push_str("          ' CHANGELOG.md > release-notes.md || { echo \"CHANGELOG.md missing section for $VERSION\" >&2; exit 1; }\n");
    } else {
        w.push_str("          : > release-notes.md\n");
    }
    writeln!(
        w,
        "          release_payload=$(jq -n --arg tag \"$VERSION\" --arg name \"$VERSION\" --arg branch \"{branch}\" --argjson prerelease \"$IS_PRERELEASE\" --rawfile body release-notes.md '{{tag_name: $tag, target_commitish: $branch, name: $name, body: $body, draft: false, prerelease: $prerelease}}')",
        branch = codeberg.target_branch
    )
    .expect("write");
    w.push_str("          status=$(curl -sS -o release.json -w '%{http_code}' -H \"Authorization: token ${CODEBERG_TOKEN}\" -H 'Content-Type: application/json' -d \"$release_payload\" \"${CODEBERG_API}/repos/${CODEBERG_REPO}/releases\")\n");
    w.push_str("          if [ \"$status\" = \"409\" ]; then\n");
    w.push_str("            curl -sS --fail -H \"Authorization: token ${CODEBERG_TOKEN}\" \"${CODEBERG_API}/repos/${CODEBERG_REPO}/releases/tags/${VERSION}\" > release.json\n");
    w.push_str("          elif [ \"$status\" -lt 200 ] || [ \"$status\" -ge 300 ]; then cat release.json; exit 1; fi\n");
    w.push_str(
        "          release_id=\"$(jq -r '.id' release.json)\"; test \"$release_id\" != \"null\"\n",
    );
    w.push_str("          shopt -s nullglob\n");
    w.push_str("          files=()\n");
    w.push_str("          add_matches() { local pattern=\"$1\" match; while IFS= read -r match; do files+=(\"$match\"); done < <(compgen -G \"$pattern\" || true); }\n");
    w.push_str("          add_matches 'release/SHA256SUMS.txt'\n");
    w.push_str("          add_matches 'release/SHA256SUMS.txt.minisig'\n");
    if artifacts.checksum_globs.is_empty() {
        for glob in [
            "*.tar.gz",
            "*.zip",
            "*.AppImage",
            "*.deb",
            "*.src.rpm",
            "*.exe",
        ] {
            writeln!(
                w,
                "          add_matches {}",
                shell_single_quote(&format!("release/{glob}"))
            )
            .expect("write");
        }
    } else {
        for glob in &artifacts.checksum_globs {
            writeln!(
                w,
                "          add_matches {}",
                shell_single_quote(&format!("release/{glob}"))
            )
            .expect("write");
        }
    }
    w.push_str("          while IFS= read -r file; do\n");
    w.push_str("            [ -f \"$file\" ] || continue\n");
    w.push_str("            name=\"$(basename \"$file\")\"\n");
    w.push_str("            asset_id=\"$(curl -sS --fail -H \"Authorization: token ${CODEBERG_TOKEN}\" \"${CODEBERG_API}/repos/${CODEBERG_REPO}/releases/${release_id}/assets\" | jq -r --arg name \"$name\" '.[] | select(.name == $name) | .id' | head -n 1)\"\n");
    w.push_str("            if [ -n \"$asset_id\" ]; then curl -sS --fail -X DELETE -H \"Authorization: token ${CODEBERG_TOKEN}\" \"${CODEBERG_API}/repos/${CODEBERG_REPO}/releases/${release_id}/assets/${asset_id}\"; fi\n");
    w.push_str("            curl -sS --fail -H \"Authorization: token ${CODEBERG_TOKEN}\" -H 'Content-Type: application/octet-stream' --data-binary \"@${file}\" \"${CODEBERG_API}/repos/${CODEBERG_REPO}/releases/${release_id}/assets?name=${name}\" > /dev/null\n");
    w.push_str(
        "          done < <(printf '%s\\n' \"${files[@]}\" | LC_ALL=C sort -u)\n          SCRIPT\n",
    );
}

/// Emit the stable-only guard shared by downstream package-repo steps.
fn push_stable_guard(w: &mut String, channel: &str) {
    w.push_str("          . ./release-env\n");
    writeln!(
        w,
        "          if [ \"$IS_PRERELEASE\" = \"true\" ]; then echo \"Prerelease ${{VERSION}}; skipping {channel}.\"; exit 0; fi"
    )
    .expect("write");
}

fn push_publisher_state_probe(w: &mut String, inputs: &ReleaseWorkflowInputs<'_>) {
    w.push_str("      - name: Probe downstream publisher state\n");
    w.push_str("        run: |\n          set -euo pipefail\n          . ./release-env\n");
    w.push_str("          : > release-publisher-state.env\n");
    w.push_str("          write_state() {\n");
    w.push_str("            channel=\"$1\"; state=\"$2\"; detail=\"$3\"\n");
    w.push_str("            var=\"PUBLISH_$(printf '%s' \"$channel\" | tr '[:lower:]-' '[:upper:]_')_STATE\"\n");
    w.push_str(
        "            printf '%s=%s\\n' \"$var\" \"$state\" >> release-publisher-state.env\n",
    );
    w.push_str("            printf '%-10s %s %s\\n' \"$channel\" \"$state\" \"$detail\"\n");
    w.push_str("          }\n");
    w.push_str("          public_git_url() {\n");
    w.push_str("            case \"$1\" in\n");
    w.push_str("              ssh://git@codeberg.org/*) path=\"${1#ssh://git@codeberg.org/}\"; printf 'https://codeberg.org/%s\\n' \"$path\" ;;\n");
    w.push_str("              git@codeberg.org:*) path=\"${1#git@codeberg.org:}\"; printf 'https://codeberg.org/%s\\n' \"$path\" ;;\n");
    w.push_str("              ssh://git@github.com/*) path=\"${1#ssh://git@github.com/}\"; printf 'https://github.com/%s\\n' \"$path\" ;;\n");
    w.push_str("              git@github.com:*) path=\"${1#git@github.com:}\"; printf 'https://github.com/%s\\n' \"$path\" ;;\n");
    w.push_str("              *) printf '%s\\n' \"$1\" ;;\n");
    w.push_str("            esac\n");
    w.push_str("          }\n");
    w.push_str("          probe_git_file() {\n");
    w.push_str("            repo=\"$1\"; ref=\"$2\"; path=\"$3\"\n");
    w.push_str("            tmp=\"$(mktemp -d)\"\n");
    w.push_str("            trap 'rm -rf \"$tmp\"' RETURN\n");
    w.push_str("            git clone --depth 1 --branch \"$ref\" \"$(public_git_url \"$repo\")\" \"$tmp/repo\" >/dev/null 2>&1 && test -e \"$tmp/repo/$path\"\n");
    w.push_str("          }\n");
    w.push_str("          probe_git_tree() {\n");
    w.push_str(
        "            repo=\"$1\"; ref=\"$2\"; path_a=\"$3\"; path_b=\"$4\"; tmp=\"$(mktemp -d)\"\n",
    );
    w.push_str("            trap 'rm -rf \"$tmp\"' RETURN\n");
    w.push_str("            git clone --depth 1 --branch \"$ref\" \"$(public_git_url \"$repo\")\" \"$tmp/repo\" >/dev/null 2>&1 && test -e \"$tmp/repo/$path_a\" && test -e \"$tmp/repo/$path_b\"\n");
    w.push_str("          }\n");
    w.push_str("          active_or_soft() {\n");
    w.push_str("            channel=\"$1\"; detail=\"$2\"; shift 2\n");
    w.push_str("            if \"$@\"; then write_state \"$channel\" active-required \"$detail\"; else write_state \"$channel\" inactive-soft \"$detail\"; fi\n");
    w.push_str("          }\n");
    w.push_str("          stable_or_state() {\n");
    w.push_str("            channel=\"$1\"; detail=\"$2\"; shift 2\n");
    w.push_str("            if [ \"$IS_PRERELEASE\" = \"true\" ]; then write_state \"$channel\" skipped-prerelease \"$detail\"; else active_or_soft \"$channel\" \"$detail\" \"$@\"; fi\n");
    w.push_str("          }\n");
    if let Some(apt) = inputs.apt {
        writeln!(
            w,
            "          stable_or_state apt {} probe_git_tree {} {} dists pool",
            shell_quote(&format!("{} {}", apt.repo_url, apt.branch)),
            shell_quote(&apt.repo_url),
            shell_quote(&apt.branch)
        )
        .expect("write");
    } else {
        w.push_str("          write_state apt not-configured '-'\n");
    }
    if let Some(aur) = inputs.aur {
        let pkgs = aur_pkg_dirs(aur)
            .into_iter()
            .map(|pkg| format!("https://aur.archlinux.org/{pkg}.git"))
            .collect::<Vec<_>>();
        let checks = pkgs
            .iter()
            .map(|repo| {
                format!(
                    "git ls-remote --exit-code {} HEAD >/dev/null 2>&1",
                    shell_quote(repo)
                )
            })
            .collect::<Vec<_>>()
            .join(" && ");
        if aur.stable_only {
            writeln!(
                w,
                "          stable_or_state aur {} bash -c {}",
                shell_quote(&aur.name),
                shell_quote(&checks)
            )
            .expect("write");
        } else {
            writeln!(
                w,
                "          active_or_soft aur {} bash -c {}",
                shell_quote(&aur.name),
                shell_quote(&checks)
            )
            .expect("write");
        }
    } else {
        w.push_str("          write_state aur not-configured '-'\n");
    }
    if let Some(copr) = inputs.copr {
        let stable_project = copr.project.as_deref().unwrap_or("");
        let testing_project = copr.testing_project.as_deref().unwrap_or(stable_project);
        writeln!(
            w,
            "          COPR_PROBE_PROJECT={}",
            shell_quote(stable_project)
        )
        .expect("write");
        writeln!(
            w,
            "          if [ \"$IS_PRERELEASE\" = \"true\" ]; then COPR_PROBE_PROJECT={}; fi",
            shell_quote(testing_project)
        )
        .expect("write");
        w.push_str("          active_or_soft copr \"$COPR_PROBE_PROJECT\" curl -fsSL \"https://copr.fedorainfracloud.org/coprs/${COPR_PROBE_PROJECT}/\"\n");
    } else {
        w.push_str("          write_state copr not-configured '-'\n");
    }
    if let Some(chocolatey) = inputs.chocolatey {
        let filter = format!(
            "Id eq '{}' and IsLatestVersion",
            chocolatey.id.replace('\'', "''")
        );
        writeln!(
            w,
            "          export CHOCO_FILTER=\"$(printf {} | jq -sRr @uri)\"",
            shell_quote(&filter)
        )
        .expect("write");
        writeln!(
            w,
            "          stable_or_state chocolatey {} bash -c 'curl -fsSL \"https://community.chocolatey.org/api/v2/Packages()?%24filter=${{CHOCO_FILTER}}\" | grep -q \"<entry>\"'",
            shell_quote(&chocolatey.id)
        )
        .expect("write");
    } else {
        w.push_str("          write_state chocolatey not-configured '-'\n");
    }
    if let Some(homebrew) = inputs.homebrew {
        writeln!(
            w,
            "          stable_or_state homebrew {} probe_git_file {} HEAD {}",
            shell_quote(&homebrew.tap_url),
            shell_quote(&homebrew.tap_url),
            shell_quote(&format!("Formula/{}.rb", homebrew.name))
        )
        .expect("write");
    } else {
        w.push_str("          write_state homebrew not-configured '-'\n");
    }
    if let Some(scoop) = inputs.scoop {
        writeln!(
            w,
            "          stable_or_state scoop {} probe_git_file {} HEAD {}",
            shell_quote(&scoop.bucket_url),
            shell_quote(&scoop.bucket_url),
            shell_quote(&format!("bucket/{}.json", scoop.name))
        )
        .expect("write");
    } else {
        w.push_str("          write_state scoop not-configured '-'\n");
    }
    if let Some(flatpak) = inputs.flatpak {
        writeln!(
            w,
            "          stable_or_state flathub {} curl -fsSL {}",
            shell_quote(&flatpak.repo),
            shell_quote(&format!("https://api.github.com/repos/{}", flatpak.repo))
        )
        .expect("write");
    } else {
        w.push_str("          write_state flathub not-configured '-'\n");
    }
    if let Some(winget) = inputs.winget {
        writeln!(
            w,
            "          stable_or_state winget {} curl -fsSL {}",
            shell_quote(&winget.package_id),
            shell_quote(&format!(
                "https://api.github.com/repos/microsoft/winget-pkgs/contents/{}",
                winget_manifest_dir(&winget.package_id)
            ))
        )
        .expect("write");
    } else {
        w.push_str("          write_state winget not-configured '-'\n");
    }
}

fn push_publisher_state_source(w: &mut String) {
    w.push_str("          if [ -f release-publisher-state.env ]; then . ./release-publisher-state.env; fi\n");
}

fn push_publisher_missing_helper(w: &mut String) {
    w.push_str("          publisher_missing() {\n");
    w.push_str("            channel=\"$1\"; message=\"$2\"\n");
    w.push_str("            var=\"PUBLISH_$(printf '%s' \"$channel\" | tr '[:lower:]-' '[:upper:]_')_STATE\"\n");
    w.push_str("            state=\"${!var:-inactive-soft}\"\n");
    w.push_str("            if [ \"$state\" = active-required ]; then echo \"::error::${channel}: ${message}\" >&2; exit 1; fi\n");
    w.push_str("            echo \"::error::${channel}: ${message}; configured publisher cannot run without required credentials/artifacts.\" >&2\n");
    w.push_str("            exit 1\n");
    w.push_str("          }\n");
}

fn push_active_or_skip_prelude(w: &mut String) {
    push_publisher_state_source(w);
    push_publisher_missing_helper(w);
}

fn push_publish_apt(w: &mut String, apt: &ResolvedApt, activated_remote: bool) {
    w.push_str("      - name: Publish APT repository\n        env:\n");
    writeln!(
        w,
        "          APT_REPO_GPG_KEY: ${{{{ secrets.{} }}}}",
        apt.gpg_key_secret
    )
    .expect("write");
    writeln!(
        w,
        "          APT_REPO_GPG_KEY_ID: ${{{{ vars.{} }}}}",
        apt.gpg_key_id_secret
    )
    .expect("write");
    writeln!(
        w,
        "          APT_REPO_GPG_PASSPHRASE: ${{{{ secrets.{} }}}}",
        apt.gpg_passphrase_secret
    )
    .expect("write");
    writeln!(
        w,
        "          APT_REPO_SSH_KEY: ${{{{ secrets.{} }}}}",
        apt.ssh_key_secret
    )
    .expect("write");
    writeln!(w, "          APT_REPO_REMOTE: {}", apt.repo_url).expect("write");
    writeln!(w, "          APT_REPO_BRANCH: {}", apt.branch).expect("write");
    writeln!(w, "          APT_DISTRIBUTION: {}", apt.distribution).expect("write");
    w.push_str("        run: |\n          set -euo pipefail\n");
    push_stable_guard(w, "APT repository publish");
    if activated_remote {
        push_active_or_skip_prelude(w);
        w.push_str("          if [ -z \"${APT_REPO_GPG_KEY:-}\" ] || [ -z \"${APT_REPO_GPG_KEY_ID:-}\" ] || [ -z \"${APT_REPO_SSH_KEY:-}\" ]; then publisher_missing apt 'apt secrets unset'; fi\n");
        w.push_str("          debs=(release/*.deb); if [ ! -e \"${debs[0]}\" ]; then publisher_missing apt 'no .deb to publish'; fi\n");
    } else {
        w.push_str("          if [ -z \"${APT_REPO_GPG_KEY:-}\" ] || [ -z \"${APT_REPO_GPG_KEY_ID:-}\" ] || [ -z \"${APT_REPO_SSH_KEY:-}\" ]; then echo '::error::apt secrets are required because APT repository publishing is configured.' >&2; exit 1; fi\n");
        w.push_str("          debs=(release/*.deb); if [ ! -e \"${debs[0]}\" ]; then echo '::error::release .deb artifacts are required because APT repository publishing is configured.' >&2; exit 1; fi\n");
    }
    w.push_str("          nix shell nixpkgs#reprepro nixpkgs#gnupg nixpkgs#openssh nixpkgs#git -c bash <<'SCRIPT'\n");
    w.push_str(
        "          set -euo pipefail\n          work=\"$(mktemp -d)\"; chmod 700 \"$work\"\n",
    );
    w.push_str("          trap 'rm -rf \"$work\"; [ -n \"${SSH_AGENT_PID:-}\" ] && ssh-agent -k >/dev/null 2>&1 || true' EXIT\n");
    w.push_str("          export GNUPGHOME=\"$work/gpg\"; mkdir -p \"$GNUPGHOME\"; chmod 700 \"$GNUPGHOME\"\n");
    w.push_str("          printf '%s' \"$APT_REPO_GPG_KEY\" | gpg --batch --import\n");
    w.push_str("          echo \"${APT_REPO_GPG_KEY_ID}:6:\" | gpg --batch --import-ownertrust\n");
    w.push_str("          eval \"$(ssh-agent -s)\"; ssh_key=\"$work/id\"; printf '%s\\n' \"$APT_REPO_SSH_KEY\" > \"$ssh_key\"; chmod 600 \"$ssh_key\"; ssh-add \"$ssh_key\"\n");
    w.push_str("          ssh_host=codeberg.org; case \"$APT_REPO_REMOTE\" in *github.com*) ssh_host=github.com ;; esac\n");
    w.push_str("          ssh_known=\"$work/known_hosts\"; ssh-keyscan \"$ssh_host\" > \"$ssh_known\" 2>/dev/null\n");
    w.push_str("          export GIT_SSH_COMMAND=\"ssh -i $ssh_key -o IdentitiesOnly=yes -o UserKnownHostsFile=$ssh_known -o StrictHostKeyChecking=yes\"\n");
    w.push_str("          mkdir -p \"$work/apt/conf\"; cp dist/apt/conf/distributions \"$work/apt/conf/distributions\"\n");
    w.push_str("          for deb in release/*.deb; do reprepro -b \"$work/apt\" includedeb \"$APT_DISTRIBUTION\" \"$deb\"; done\n");
    w.push_str("          git clone --depth 1 --branch \"$APT_REPO_BRANCH\" \"$APT_REPO_REMOTE\" \"$work/checkout\"\n");
    w.push_str("          shopt -s dotglob nullglob\n");
    w.push_str("          for path in \"$work/checkout\"/*; do name=\"$(basename \"$path\")\"; case \"$name\" in .git|README.md) continue ;; esac; rm -rf \"$path\"; done\n");
    w.push_str("          shopt -u dotglob nullglob\n");
    w.push_str("          cp -r \"$work/apt/dists\" \"$work/checkout/dists\"; cp -r \"$work/apt/pool\" \"$work/checkout/pool\"\n");
    w.push_str("          if [ -f dist/apt/key.gpg.asc ]; then cp dist/apt/key.gpg.asc \"$work/checkout/key.gpg.asc\"; fi\n");
    w.push_str("          cd \"$work/checkout\"\n");
    w.push_str(
        "          git -c user.name='release bot' -c user.email='release-bot@localhost' add -A\n",
    );
    w.push_str("          if git diff --cached --quiet; then echo 'apt: no changes'; exit 0; fi\n");
    w.push_str("          git -c user.name='release bot' -c user.email='release-bot@localhost' commit -m \"apt: publish ${VERSION}\"\n");
    w.push_str(
        "          git push --force-with-lease origin \"HEAD:refs/heads/${APT_REPO_BRANCH}\"\n",
    );
    w.push_str("          SCRIPT\n");
}

fn push_publish_aur(w: &mut String, aur: &ResolvedAur, activated_remote: bool) {
    w.push_str("      - name: Publish AUR packages\n        env:\n");
    writeln!(
        w,
        "          AUR_SSH_KEY: ${{{{ secrets.{} }}}}",
        aur.ssh_key_secret
    )
    .expect("write");
    w.push_str("        run: |\n          set -euo pipefail\n");
    if aur.stable_only {
        push_stable_guard(w, "AUR packages");
    } else {
        w.push_str("          . ./release-env\n");
    }
    if activated_remote {
        push_active_or_skip_prelude(w);
        w.push_str("          if [ -z \"${AUR_SSH_KEY:-}\" ]; then publisher_missing aur 'AUR_SSH_KEY unset'; fi\n");
    } else {
        w.push_str("          test -n \"${AUR_SSH_KEY:-}\"\n");
    }
    w.push_str(
        "          nix shell nixpkgs#git nixpkgs#openssh nixpkgs#pacman -c bash <<'SCRIPT'\n",
    );
    w.push_str("          set -euo pipefail\n          . ./release-env\n");
    w.push_str("          AUR_HOST='aur.archlinux.org'; AUR_SSH=\"$HOME/.ssh/aur\"\n");
    w.push_str("          MAKEPKG_CONF=\"$(dirname \"$(dirname \"$(command -v makepkg)\")\")/etc/makepkg.conf\"; test -r \"$MAKEPKG_CONF\"\n");
    w.push_str("          checksum_for() { awk -v a=\"$1\" '$2 == a { print $1 }' release/SHA256SUMS.txt; }\n");
    let source_archive = pkgbuild_source_archive(aur);
    let bin_archive = pkgbuild_binary_archive(aur);
    writeln!(
        w,
        "          source_sha=\"$(checksum_for \"{source_archive}\")\""
    )
    .expect("write");
    if aur.flavors.bin {
        writeln!(
            w,
            "          bin_sha=\"$(checksum_for \"{bin_archive}\")\"; test -n \"$bin_sha\""
        )
        .expect("write");
    }
    w.push_str("          test -n \"$source_sha\"\n");
    w.push_str("          install -d -m 700 \"$HOME/.ssh\"; printf '%s\\n' \"$AUR_SSH_KEY\" > \"$AUR_SSH\"; chmod 600 \"$AUR_SSH\"\n");
    w.push_str("          ssh-keyscan \"$AUR_HOST\" >> \"$HOME/.ssh/known_hosts\"; chmod 600 \"$HOME/.ssh/known_hosts\"\n");
    w.push_str("          export GIT_SSH_COMMAND=\"ssh -i $AUR_SSH -o IdentitiesOnly=yes\"\n");
    w.push_str("          publish_pkg() {\n            pkg=\"$1\"\n");
    writeln!(w, "            repo=\"{}/${{pkg}}.git\"", aur.ssh_remote).expect("write");
    w.push_str("            rm -rf \"aur-${pkg}\"\n");
    w.push_str("            if ! git clone \"$repo\" \"aur-${pkg}\"; then mkdir \"aur-${pkg}\"; git -C \"aur-${pkg}\" init; git -C \"aur-${pkg}\" remote add origin \"$repo\"; fi\n");
    w.push_str("            git -C \"aur-${pkg}\" checkout -B master\n");
    w.push_str("            cp \"dist/aur/${pkg}/PKGBUILD\" \"aur-${pkg}/PKGBUILD\"\n");
    w.push_str("            case \"$pkg\" in\n");
    writeln!(w, "              {})", aur.name).expect("write");
    w.push_str("                sed -i -e \"s/^pkgver=.*/pkgver=${VERSION}/\" -e 's/^pkgrel=.*/pkgrel=1/' \"aur-${pkg}/PKGBUILD\"\n");
    w.push_str("                awk -v sha=\"$source_sha\" '/^sha256sums=/{print \"sha256sums=(\\047\" sha \"\\047)\"; next} {print}' \"aur-${pkg}/PKGBUILD\" > \"aur-${pkg}/PKGBUILD.new\"; mv \"aur-${pkg}/PKGBUILD.new\" \"aur-${pkg}/PKGBUILD\" ;;\n");
    if aur.flavors.bin {
        writeln!(w, "              {}-bin)", aur.name).expect("write");
        w.push_str("                sed -i -e \"s/^pkgver=.*/pkgver=${VERSION}/\" -e 's/^pkgrel=.*/pkgrel=1/' \"aur-${pkg}/PKGBUILD\"\n");
        w.push_str("                awk -v b=\"$bin_sha\" -v s=\"$source_sha\" '/^sha256sums=/{print \"sha256sums=(\\047\" b \"\\047\"; print \"            \\047\" s \"\\047)\"; skip=1; next} skip && /^[[:space:]]*\\047/{next} {skip=0; print}' \"aur-${pkg}/PKGBUILD\" > \"aur-${pkg}/PKGBUILD.new\"; mv \"aur-${pkg}/PKGBUILD.new\" \"aur-${pkg}/PKGBUILD\" ;;\n");
    }
    if aur.flavors.git {
        writeln!(w, "              {}-git) ;;", aur.name).expect("write");
    }
    w.push_str(
        "              *) echo \"unknown AUR package: $pkg\" >&2; exit 1 ;;\n            esac\n",
    );
    w.push_str("            (cd \"aur-${pkg}\" && makepkg --config \"$MAKEPKG_CONF\" --printsrcinfo > .SRCINFO)\n");
    w.push_str("            git -C \"aur-${pkg}\" add PKGBUILD .SRCINFO\n");
    w.push_str("            if git -C \"aur-${pkg}\" diff --cached --quiet; then echo \"AUR ${pkg} already at ${VERSION}\"; return 0; fi\n");
    w.push_str("            git -C \"aur-${pkg}\" -c user.email='ci@localhost' -c user.name='release bot' commit -m \"${VERSION}\"\n");
    w.push_str("            git -C \"aur-${pkg}\" push origin HEAD:master\n          }\n");
    let pkgs = aur_pkg_dirs(aur).join(" ");
    writeln!(
        w,
        "          for pkg in {pkgs}; do publish_pkg \"$pkg\"; done"
    )
    .expect("write");
    w.push_str("          SCRIPT\n");
}

fn push_publish_copr(w: &mut String, copr: &ResolvedCopr, activated_remote: bool) {
    w.push_str("      - name: Push SRPM to COPR\n        env:\n");
    writeln!(
        w,
        "          COPR_LOGIN: ${{{{ secrets.{} }}}}",
        copr.login_secret
    )
    .expect("write");
    writeln!(
        w,
        "          COPR_USERNAME: ${{{{ vars.{} }}}}",
        copr.username_secret
    )
    .expect("write");
    writeln!(
        w,
        "          COPR_TOKEN: ${{{{ secrets.{} }}}}",
        copr.token_secret
    )
    .expect("write");
    w.push_str("        run: |\n          set -euo pipefail\n          . ./release-env\n");
    let stable_project = copr.project.as_deref().unwrap_or("");
    let testing_project = copr.testing_project.as_deref().unwrap_or(stable_project);
    writeln!(w, "          COPR_PROJECT=\"{stable_project}\"").expect("write");
    writeln!(w, "          if [ \"$IS_PRERELEASE\" = \"true\" ]; then COPR_PROJECT=\"{testing_project}\"; fi").expect("write");
    if activated_remote {
        push_active_or_skip_prelude(w);
        w.push_str("          if [ -z \"$COPR_LOGIN\" ] || [ -z \"$COPR_USERNAME\" ] || [ -z \"$COPR_TOKEN\" ]; then publisher_missing copr 'COPR credentials unset'; fi\n");
        w.push_str("          srpms=(srpms/*.src.rpm); if [ ! -e \"${srpms[0]}\" ]; then publisher_missing copr 'no SRPM to publish'; fi\n");
    } else {
        w.push_str("          if [ -z \"$COPR_LOGIN\" ] || [ -z \"$COPR_USERNAME\" ] || [ -z \"$COPR_TOKEN\" ]; then echo 'COPR credentials are required because COPR publishing is configured.' >&2; exit 1; fi\n");
    }
    w.push_str("          mkdir -p ~/.config\n");
    w.push_str("          cat > ~/.config/copr <<EOF\n          [copr-cli]\n          login = ${COPR_LOGIN}\n          username = ${COPR_USERNAME}\n          token = ${COPR_TOKEN}\n          copr_url = https://copr.fedorainfracloud.org\n          EOF\n");
    w.push_str("          chmod 600 ~/.config/copr\n");
    writeln!(
        w,
        "          nix run {} -- build --nowait \"${{COPR_PROJECT}}\" srpms/*.src.rpm",
        copr.nix_tool
    )
    .expect("write");
}

fn push_publish_homebrew(
    w: &mut String,
    homebrew: &ResolvedHomebrew,
    activated_remote: bool,
    platform: Platform,
) {
    w.push_str("      - name: Publish Homebrew tap\n        env:\n");
    writeln!(
        w,
        "          HOMEBREW_TAP_TOKEN: ${{{{ secrets.{} }}}}",
        homebrew.tap_token_secret
    )
    .expect("write");
    writeln!(w, "          HOMEBREW_TAP_URL: {}", homebrew.tap_url).expect("write");
    w.push_str("        run: |\n          set -euo pipefail\n");
    push_stable_guard(w, "Homebrew tap update");
    if activated_remote {
        push_active_or_skip_prelude(w);
        w.push_str("          if [ -z \"${HOMEBREW_TAP_TOKEN:-}\" ]; then publisher_missing homebrew 'HOMEBREW_TAP_TOKEN unset'; fi\n");
        for (_, arch, os, enabled) in homebrew_platform_keys(homebrew) {
            if enabled {
                let file = homebrew_archive(homebrew, arch, os);
                writeln!(w, "          test -s release/{file} || publisher_missing homebrew 'missing release/{file}'").expect("write");
            }
        }
    } else {
        w.push_str("          if [ -z \"${HOMEBREW_TAP_TOKEN:-}\" ]; then echo 'HOMEBREW_TAP_TOKEN is required because Homebrew tap publishing is configured.' >&2; exit 1; fi\n");
    }
    w.push_str("          credential_helper='!f() { echo username=x-access-token; echo \"password=$HOMEBREW_TAP_TOKEN\"; }; f'\n");
    w.push_str("          rm -rf tap; git -c credential.helper=\"$credential_helper\" clone \"$HOMEBREW_TAP_URL\" tap\n");
    w.push_str("          cd tap; git config credential.helper \"$credential_helper\"; git config user.email 'ci@localhost'; git config user.name 'release bot'\n");
    w.push_str("          git remote set-head origin -a; DEFAULT_BRANCH=\"$(git symbolic-ref --short refs/remotes/origin/HEAD | sed 's|^origin/||')\"; git checkout \"$DEFAULT_BRANCH\"; cd ..\n");
    w.push_str("          nix run '.#rs-harbor' -- brew bump \\\n");
    writeln!(w, "            --name {} \\", homebrew.name).expect("write");
    w.push_str("            --version \"$VERSION\" \\\n");
    writeln!(
        w,
        "            --description {} \\",
        shell_quote(&homebrew.description)
    )
    .expect("write");
    writeln!(
        w,
        "            --homepage {} \\",
        shell_quote(&homebrew.homepage)
    )
    .expect("write");
    writeln!(w, "            --license {} \\", homebrew.license).expect("write");
    for (key, arch, os, enabled) in homebrew_platform_keys(homebrew) {
        if !enabled {
            continue;
        }
        let file = homebrew_archive(homebrew, arch, os);
        writeln!(
            w,
            "            --archive \"{key}={host}/{repo}/releases/download/${{VERSION}}/{file},release/{file}\" \\",
            host = platform.web_base_url(),
            repo = homebrew.download_repo
        )
        .expect("write");
    }
    for binary in &homebrew.binaries {
        writeln!(w, "            --binary {binary} \\").expect("write");
    }
    w.push_str("            --tap \"$PWD/tap\"\n");
    w.push_str("          cd tap\n");
    writeln!(w, "          if [ -z \"$(git status --porcelain -- Formula/{}.rb)\" ]; then echo 'tap unchanged'; exit 0; fi", homebrew.name).expect("write");
    writeln!(w, "          git add Formula/{}.rb; git commit -m \"{} ${{VERSION}}\"; git push origin \"HEAD:${{DEFAULT_BRANCH}}\"", homebrew.name, homebrew.name).expect("write");
}

fn push_publish_scoop(w: &mut String, scoop: &ResolvedScoop, activated_remote: bool) {
    w.push_str("      - name: Publish Scoop bucket\n        env:\n");
    writeln!(
        w,
        "          SCOOP_BUCKET_TOKEN: ${{{{ secrets.{} }}}}",
        scoop.bucket_token_secret
    )
    .expect("write");
    writeln!(w, "          SCOOP_BUCKET_URL: {}", scoop.bucket_url).expect("write");
    w.push_str("        run: |\n          set -euo pipefail\n");
    push_stable_guard(w, "Scoop bucket update");
    if activated_remote {
        push_active_or_skip_prelude(w);
        w.push_str("          if [ -z \"${SCOOP_BUCKET_TOKEN:-}\" ]; then publisher_missing scoop 'SCOOP_BUCKET_TOKEN unset'; fi\n");
    } else {
        w.push_str("          if [ -z \"${SCOOP_BUCKET_TOKEN:-}\" ]; then echo 'SCOOP_BUCKET_TOKEN is required because Scoop bucket publishing is configured.' >&2; exit 1; fi\n");
    }
    let zip = scoop_windows_zip(scoop);
    if activated_remote {
        writeln!(w, "          ZIP_NAME=\"{zip}\"; test -s \"release/${{ZIP_NAME}}\" || publisher_missing scoop \"missing release/${{ZIP_NAME}}\"; test -s dist/scoop/{}.json || publisher_missing scoop 'missing dist/scoop/{}.json'", scoop.name, scoop.name).expect("write");
    } else {
        writeln!(w, "          ZIP_NAME=\"{zip}\"; test -s \"release/${{ZIP_NAME}}\"; test -s dist/scoop/{}.json", scoop.name).expect("write");
    }
    w.push_str("          SHA256=\"$(awk -v a=\"$ZIP_NAME\" '$2 == a { print $1 }' release/SHA256SUMS.txt)\"; test -n \"$SHA256\"\n");
    w.push_str("          credential_helper='!f() { echo username=x-access-token; echo \"password=$SCOOP_BUCKET_TOKEN\"; }; f'\n");
    w.push_str("          rm -rf scoop-bucket; git -c credential.helper=\"$credential_helper\" clone \"$SCOOP_BUCKET_URL\" scoop-bucket\n");
    w.push_str("          cd scoop-bucket; git config credential.helper \"$credential_helper\"; git config user.email 'ci@localhost'; git config user.name 'release bot'\n");
    w.push_str("          git remote set-head origin -a; DEFAULT_BRANCH=\"$(git symbolic-ref --short refs/remotes/origin/HEAD | sed 's|^origin/||')\"; git checkout \"$DEFAULT_BRANCH\"; mkdir -p bucket; cd ..\n");
    writeln!(w, "          sed -e \"s|{{{{VERSION}}}}|${{VERSION}}|g\" -e \"s|{{{{SHA256}}}}|${{SHA256}}|g\" dist/scoop/{name}.json > scoop-bucket/bucket/{name}.json", name = scoop.name).expect("write");
    writeln!(w, "          cd scoop-bucket; if [ -z \"$(git status --porcelain -- bucket/{name}.json)\" ]; then echo 'bucket unchanged'; exit 0; fi", name = scoop.name).expect("write");
    writeln!(w, "          git add bucket/{name}.json; git commit -m \"{name} ${{VERSION}}\"; git push origin \"HEAD:${{DEFAULT_BRANCH}}\"", name = scoop.name).expect("write");
}

fn push_sbom(w: &mut String, commands: &[String]) {
    w.push_str("      - name: Generate supply-chain reports\n        run: |\n          set -euo pipefail\n          . ./release-env\n          export VERSION IS_PRERELEASE\n          mkdir -p release\n");
    for line in commands {
        for sub in line.split('\n') {
            writeln!(w, "          {sub}").expect("write");
        }
    }
}

fn push_windows_signing(w: &mut String, windows: &WindowsSigningConfig) {
    let dir = &windows.dir;
    let tar = windows.tar_archive.replace("{version}", "${VERSION}");
    let zip = windows.zip_archive.replace("{version}", "${VERSION}");
    w.push_str("      - name: Sign Windows Authenticode artifacts\n        env:\n");
    writeln!(
        w,
        "          WINDOWS_SIGNING_PFX: ${{{{ secrets.{} }}}}",
        windows.pfx_secret
    )
    .expect("write");
    writeln!(
        w,
        "          WINDOWS_SIGNING_PASS: ${{{{ secrets.{} }}}}",
        windows.pass_secret
    )
    .expect("write");
    writeln!(
        w,
        "          WINDOWS_SIGNING_SUBJECT: ${{{{ secrets.{} }}}}",
        windows.subject_secret
    )
    .expect("write");
    w.push_str("        run: |\n          set -euo pipefail\n          . ./release-env\n");
    let exes = windows
        .binaries
        .iter()
        .map(|b| format!("{dir}/{b}.exe"))
        .collect::<Vec<_>>()
        .join(" ");
    writeln!(w, "          for exe in {exes}; do test -s \"$exe\"; done").expect("write");
    w.push_str("          if [ -n \"${WINDOWS_SIGNING_PFX:-}\" ] && [ -n \"${WINDOWS_SIGNING_PASS:-}\" ]; then\n");
    w.push_str("            nix shell nixpkgs#osslsigncode nixpkgs#openssl -c bash <<'SCRIPT'\n");
    w.push_str("          set -euo pipefail\n          umask 077\n");
    w.push_str("          pfx_file=\"$(mktemp)\"; pass_file=\"$(mktemp)\"\n");
    w.push_str("          trap 'rm -f \"$pfx_file\" \"$pass_file\"' EXIT\n");
    w.push_str(
        "          printf '%s' \"$WINDOWS_SIGNING_PFX\" | base64 --decode > \"$pfx_file\"\n",
    );
    w.push_str("          printf '%s' \"$WINDOWS_SIGNING_PASS\" > \"$pass_file\"\n");
    writeln!(w, "          for exe in {exes}; do").expect("write");
    w.push_str("            osslsigncode sign -pkcs12 \"$pfx_file\" -readpass \"$pass_file\" -h sha256 \\\n");
    writeln!(
        w,
        "              -n {} -i {} -ts {} \\",
        shell_quote(&windows.sign_name),
        shell_quote(&windows.sign_url),
        shell_quote(&windows.timestamp_url)
    )
    .expect("write");
    w.push_str("              -in \"$exe\" -out \"${exe}.signed\"\n");
    w.push_str("            osslsigncode verify -in \"${exe}.signed\"\n");
    w.push_str("            mv \"${exe}.signed\" \"$exe\"\n");
    w.push_str("          done\n          SCRIPT\n");
    w.push_str("          else\n            echo '::warning::WINDOWS_SIGNING_PFX/PASS unset; publishing unsigned Windows EXEs.'\n          fi\n");
    let basenames = windows
        .binaries
        .iter()
        .map(|b| format!("{b}.exe"))
        .collect::<Vec<_>>()
        .join(" ");
    writeln!(
        w,
        "          tar czf \"release/{tar}\" -C {dir} {basenames}"
    )
    .expect("write");
    writeln!(w, "          zip_out=\"$PWD/release/{zip}\"").expect("write");
    writeln!(
        w,
        "          nix shell nixpkgs#zip -c bash -c 'cd \"$1\" && shift && zip -q \"$1\" \"$@\"' _ {dir} \"$zip_out\" {basenames}"
    )
    .expect("write");
    // Individual signed .exe assets, e.g. modde-${VERSION}-x86_64-windows.exe.
    let platform_suffix = windows
        .zip_archive
        .split_once("{version}")
        .map(|(_, rest)| rest.trim_end_matches(".zip"))
        .unwrap_or("");
    for binary in &windows.binaries {
        writeln!(
            w,
            "          cp {dir}/{binary}.exe \"release/{binary}-${{VERSION}}{platform_suffix}.exe\""
        )
        .expect("write");
    }
}

fn push_announce(
    w: &mut String,
    announce: &AnnounceConfig,
    release: Option<&ResolvedReleaseTarget>,
    platform: Platform,
) {
    let fallback = release
        .map(|c| platform.release_tag_url(&c.repo, "${VERSION}"))
        .unwrap_or_else(|| "${VERSION}".to_owned());
    w.push_str("      - name: Announce stable release\n        env:\n");
    writeln!(
        w,
        "          MASTODON_TOKEN: ${{{{ secrets.{} }}}}",
        announce.mastodon_token_secret
    )
    .expect("write");
    writeln!(
        w,
        "          MASTODON_BASE_URL: ${{{{ secrets.{} }}}}",
        announce.mastodon_base_url_secret
    )
    .expect("write");
    writeln!(
        w,
        "          MATRIX_TOKEN: ${{{{ secrets.{} }}}}",
        announce.matrix_token_secret
    )
    .expect("write");
    writeln!(
        w,
        "          MATRIX_HOMESERVER: ${{{{ secrets.{} }}}}",
        announce.matrix_homeserver_secret
    )
    .expect("write");
    writeln!(
        w,
        "          MATRIX_ROOM: ${{{{ secrets.{} }}}}",
        announce.matrix_room_secret
    )
    .expect("write");
    w.push_str("        run: |\n          set -euo pipefail\n");
    push_stable_guard(w, "announcements");
    w.push_str("          nix shell nixpkgs#curl nixpkgs#jq -c bash <<'SCRIPT'\n");
    w.push_str("          set -euo pipefail\n          . ./release-env\n");
    w.push_str("          release_url=\"$(jq -r '.html_url // .url // empty' release.json 2>/dev/null || true)\"\n");
    writeln!(
        w,
        "          if [ -z \"$release_url\" ]; then release_url=\"{fallback}\"; fi"
    )
    .expect("write");
    w.push_str("          body=\"release ${VERSION} is out: ${release_url}\"\n");
    w.push_str(
        "          if [ -n \"${MASTODON_TOKEN:-}\" ] && [ -n \"${MASTODON_BASE_URL:-}\" ]; then\n",
    );
    w.push_str(
        "            jq -n --arg status \"$body\" '{status: $status}' > mastodon-status.json\n",
    );
    w.push_str("            curl -sS --fail -H \"Authorization: Bearer ${MASTODON_TOKEN}\" -H 'Content-Type: application/json' -d @mastodon-status.json \"${MASTODON_BASE_URL%/}/api/v1/statuses\" > /dev/null\n");
    w.push_str("          else echo 'Mastodon secrets are required because release announcements are configured.' >&2; exit 1; fi\n");
    w.push_str("          if [ -n \"${MATRIX_TOKEN:-}\" ] && [ -n \"${MATRIX_HOMESERVER:-}\" ] && [ -n \"${MATRIX_ROOM:-}\" ]; then\n");
    w.push_str("            txnid=\"release-${VERSION}-${CODEBERG_RUN_ID:-manual}\"\n");
    w.push_str("            encoded_room=\"$(jq -rn --arg v \"$MATRIX_ROOM\" '$v|@uri')\"\n");
    w.push_str("            jq -n --arg body \"$body\" '{msgtype: \"m.text\", body: $body}' > matrix-message.json\n");
    w.push_str("            curl -sS --fail -X PUT -H \"Authorization: Bearer ${MATRIX_TOKEN}\" -H 'Content-Type: application/json' -d @matrix-message.json \"${MATRIX_HOMESERVER%/}/_matrix/client/r0/rooms/${encoded_room}/send/m.room.message/${txnid}\" > /dev/null\n");
    w.push_str("          else echo 'Matrix secrets are required because release announcements are configured.' >&2; exit 1; fi\n          SCRIPT\n");
}

fn push_publish_flathub(w: &mut String, flatpak: &FlatpakConfig, activated_remote: bool) {
    w.push_str("      - name: Publish Flathub update PR\n        env:\n");
    writeln!(
        w,
        "          FLATHUB_TOKEN: ${{{{ secrets.{} }}}}",
        flatpak.token_secret
    )
    .expect("write");
    w.push_str("        run: |\n          set -euo pipefail\n");
    push_stable_guard(w, "Flathub update");
    if activated_remote {
        push_active_or_skip_prelude(w);
        w.push_str("          if [ -z \"${FLATHUB_TOKEN:-}\" ]; then publisher_missing flathub 'FLATHUB_TOKEN unset'; fi\n");
    } else {
        w.push_str("          if [ -z \"${FLATHUB_TOKEN:-}\" ]; then echo 'FLATHUB_TOKEN is required because Flathub publishing is configured.' >&2; exit 1; fi\n");
    }
    for file in &flatpak.manifest_files {
        if activated_remote {
            writeln!(w, "          test -s release/{file} || publisher_missing flathub 'missing release/{file}'").expect("write");
        } else {
            writeln!(w, "          test -s release/{file}").expect("write");
        }
    }
    w.push_str("          nix shell nixpkgs#curl nixpkgs#git nixpkgs#jq -c bash <<'SCRIPT'\n");
    w.push_str("          set -euo pipefail\n          . ./release-env\n");
    w.push_str("          credential_helper='!f() { echo username=x-access-token; echo \"password=$FLATHUB_TOKEN\"; }; f'\n");
    writeln!(w, "          rm -rf flathub-repo; git -c credential.helper=\"$credential_helper\" clone https://github.com/{}.git flathub-repo", flatpak.repo).expect("write");
    w.push_str("          cd flathub-repo\n          git config credential.helper \"$credential_helper\"; git config user.email 'ci@localhost'; git config user.name 'release bot'\n");
    w.push_str("          git checkout -B \"release/${VERSION}\"\n");
    let files = flatpak.manifest_files.join(" ");
    writeln!(
        w,
        "          cp {} .",
        flatpak
            .manifest_files
            .iter()
            .map(|f| format!("../release/{f}"))
            .collect::<Vec<_>>()
            .join(" ")
    )
    .expect("write");
    writeln!(w, "          if [ -z \"$(git status --porcelain -- {files})\" ]; then echo 'manifest unchanged'; exit 0; fi").expect("write");
    writeln!(w, "          git add {files}; git commit -m \"${{VERSION}}\"; git push --force-with-lease origin \"release/${{VERSION}}\"").expect("write");
    writeln!(w, "          payload=\"$(jq -n --arg title \"${{VERSION}}\" --arg head \"release/${{VERSION}}\" --arg base \"{base}\" --arg body \"Update to ${{VERSION}}.\" '{{title: $title, head: $head, base: $base, body: $body}}')\"", base = flatpak.base_branch).expect("write");
    writeln!(w, "          status=\"$(curl -sS -o pr.json -w '%{{http_code}}' -H \"Authorization: Bearer ${{FLATHUB_TOKEN}}\" -H 'Accept: application/vnd.github+json' -d \"$payload\" https://api.github.com/repos/{}/pulls)\"", flatpak.repo).expect("write");
    w.push_str("          if [ \"$status\" -eq 422 ]; then echo 'PR exists or rejected'; jq -r '.message' pr.json; exit 0; fi\n");
    w.push_str("          if [ \"$status\" -lt 200 ] || [ \"$status\" -ge 300 ]; then cat pr.json; exit 1; fi\n");
    w.push_str("          jq -r '.html_url' pr.json\n          SCRIPT\n");
}

fn push_publish_winget(
    w: &mut String,
    winget: &WingetConfig,
    activated_remote: bool,
    platform: Platform,
) {
    let zip = winget.zip_archive.replace("{version}", "${VERSION}");
    w.push_str("      - name: Publish winget manifest PR\n        env:\n");
    writeln!(
        w,
        "          WINGET_PAT: ${{{{ secrets.{} }}}}",
        winget.token_secret
    )
    .expect("write");
    w.push_str("        run: |\n          set -euo pipefail\n");
    push_stable_guard(w, "winget manifest update");
    if activated_remote {
        push_active_or_skip_prelude(w);
        w.push_str("          if [ -z \"${WINGET_PAT:-}\" ]; then publisher_missing winget 'WINGET_PAT unset'; fi\n");
    } else {
        w.push_str("          if [ -z \"${WINGET_PAT:-}\" ]; then echo 'WINGET_PAT is required because WinGet publishing is configured.' >&2; exit 1; fi\n");
    }
    w.push_str("          ZIP_NAME=\"");
    w.push_str(&zip);
    w.push_str("\"; ZIP_URL=\"");
    w.push_str(platform.web_base_url());
    w.push('/');
    w.push_str(&winget.download_repo);
    w.push_str("/releases/download/${VERSION}/${ZIP_NAME}\"\n");
    if activated_remote {
        w.push_str("          test -s \"release/${ZIP_NAME}\" || publisher_missing winget \"missing release/${ZIP_NAME}\"\n");
    } else {
        w.push_str("          test -s \"release/${ZIP_NAME}\"\n");
    }
    w.push_str("          export ZIP_URL VERSION WINGET_PAT\n");
    w.push_str("          nix shell nixpkgs#curl nixpkgs#jq nixpkgs#unzip nixpkgs#wineWow64Packages.stable nixpkgs#util-linux -c bash <<'SCRIPT'\n");
    w.push_str("          set -euo pipefail\n          tmpdir=\"$(mktemp -d)\"; trap 'rm -rf \"$tmpdir\"' EXIT\n");
    w.push_str(
        "          export WINEPREFIX=\"$tmpdir/wine\"; export WINEDEBUG=-all; export TERM=xterm\n",
    );
    w.push_str("          release_json=\"$(curl -fsSL https://api.github.com/repos/microsoft/winget-create/releases/latest)\"\n");
    w.push_str("          url=\"$(printf '%s' \"$release_json\" | jq -r '.assets[] | select(.name == \"wingetcreate.exe\") | .browser_download_url' | head -n 1)\"\n");
    w.push_str("          curl -fsSL -o \"$tmpdir/wingetcreate.exe\" \"$url\"\n");
    writeln!(w, "          wine \"$tmpdir/wingetcreate.exe\" update {} --version \"$VERSION\" --urls \"${{ZIP_URL}}|x64\" --token \"$WINGET_PAT\" --submit", winget.package_id).expect("write");
    w.push_str("          SCRIPT\n");
}

fn push_publish_chocolatey(
    w: &mut String,
    chocolatey: &ResolvedChocolatey,
    activated_remote: bool,
) {
    let zip = chocolatey
        .archive_pattern
        .replace("{name}", &chocolatey.name)
        .replace("{version}", "${VERSION}")
        .replace("{arch}", "x86_64");
    let key_env = &chocolatey.api_key_env;
    w.push_str("      - name: Publish Chocolatey package\n        env:\n");
    // Source the key from an Actions secret, unless the runner already provides
    // it in the job environment (e.g. a Forgejo runner credential).
    if !chocolatey.api_key_from_runner {
        writeln!(
            w,
            "          {key_env}: ${{{{ secrets.{} }}}}",
            chocolatey.api_key_secret
        )
        .expect("write");
    }
    writeln!(w, "          CHOCO_PUSH_SOURCE: {}", chocolatey.push.source).expect("write");
    w.push_str("        run: |\n          set -euo pipefail\n");
    push_stable_guard(w, "Chocolatey package update");
    if activated_remote {
        push_active_or_skip_prelude(w);
        writeln!(w, "          if [ -z \"${{{key_env}:-}}\" ]; then publisher_missing chocolatey '{key_env} unset'; fi").expect("write");
    } else {
        writeln!(w, "          if [ -z \"${{{key_env}:-}}\" ]; then echo '{key_env} is required because Chocolatey package publishing is configured.' >&2; exit 1; fi").expect("write");
    }
    // `nix_tool` provides choco (+ simit). Point it at a fork that ships the
    // chocolatey package until it lands upstream; the soft-skip above keeps tags
    // green in the meantime.
    if activated_remote {
        writeln!(w, "          ZIP=\"release/{zip}\"; test -s \"$ZIP\" || publisher_missing chocolatey \"missing $ZIP\"").expect("write");
    } else {
        writeln!(w, "          ZIP=\"release/{zip}\"; test -s \"$ZIP\"").expect("write");
    }
    writeln!(
        w,
        "          nix shell {} -c simit dist chocolatey bump \\",
        chocolatey.nix_tool
    )
    .expect("write");
    w.push_str(
        "            --version \"$VERSION\" \\\n            --package-dir chocolatey-package \\\n",
    );
    w.push_str("            --archive \"x64=$ZIP\" \\\n");
    push_choco_flag(w, "--choco-name", &chocolatey.name);
    push_choco_flag(w, "--choco-id", &chocolatey.id);
    push_choco_flag(w, "--choco-title", &chocolatey.title);
    if let Some(authors) = &chocolatey.authors {
        push_choco_flag(w, "--choco-authors", authors);
    }
    push_choco_flag(w, "--choco-description", &chocolatey.description);
    if let Some(summary) = &chocolatey.summary {
        push_choco_flag(w, "--choco-summary", summary);
    }
    push_choco_flag(w, "--choco-project-url", &chocolatey.project_url);
    if let Some(license_url) = &chocolatey.license_url {
        push_choco_flag(w, "--choco-license-url", license_url);
    }
    if let Some(icon_url) = &chocolatey.icon_url {
        push_choco_flag(w, "--choco-icon-url", icon_url);
    }
    if let Some(package_source_url) = &chocolatey.package_source_url {
        push_choco_flag(w, "--choco-package-source-url", package_source_url);
    }
    if let Some(docs_url) = &chocolatey.docs_url {
        push_choco_flag(w, "--choco-docs-url", docs_url);
    }
    if let Some(bug_tracker_url) = &chocolatey.bug_tracker_url {
        push_choco_flag(w, "--choco-bug-tracker-url", bug_tracker_url);
    }
    if let Some(project_source_url) = &chocolatey.project_source_url {
        push_choco_flag(w, "--choco-project-source-url", project_source_url);
    }
    if let Some(tags) = &chocolatey.tags {
        push_choco_flag(w, "--choco-tags", tags);
    }
    if let Some(release_notes_url) = &chocolatey.release_notes_url {
        push_choco_flag(w, "--choco-release-notes-url", release_notes_url);
    }
    push_choco_flag(w, "--choco-download-repo", &chocolatey.download_repo);
    push_choco_flag(w, "--choco-archive-pattern", &chocolatey.archive_pattern);
    writeln!(w, "            --push \\\n            --push-source \"$CHOCO_PUSH_SOURCE\" \\\n            --api-key-env {key_env}").expect("write");
}

fn push_choco_flag(w: &mut String, flag: &str, value: &str) {
    writeln!(w, "            {flag} {} \\", shell_quote(value)).expect("write");
}

// --- helpers shared with the dist renderers (archive names with literal version) ---

fn pkgbuild_source_archive(aur: &ResolvedAur) -> String {
    aur.source_archive_pattern
        .replace("{repo}", &aur.repo)
        .replace("{name}", &aur.name)
        .replace("{version}", "${VERSION}")
}

fn pkgbuild_binary_archive(aur: &ResolvedAur) -> String {
    aur.binary_archive_pattern
        .replace("{repo}", &aur.repo)
        .replace("{name}", &aur.name)
        .replace("{version}", "${VERSION}")
}

fn aur_pkg_dirs(aur: &ResolvedAur) -> Vec<String> {
    let mut dirs = Vec::new();
    if aur.flavors.source {
        dirs.push(aur.name.clone());
    }
    if aur.flavors.bin {
        dirs.push(format!("{}-bin", aur.name));
    }
    if aur.flavors.git {
        dirs.push(format!("{}-git", aur.name));
    }
    dirs
}

fn homebrew_platform_keys(
    homebrew: &ResolvedHomebrew,
) -> [(&'static str, &'static str, &'static str, bool); 4] {
    [
        (
            "darwin_arm",
            "aarch64",
            "darwin",
            homebrew.platforms.darwin_arm,
        ),
        (
            "darwin_intel",
            "x86_64",
            "darwin",
            homebrew.platforms.darwin_intel,
        ),
        (
            "linux_arm",
            "aarch64",
            "linux",
            homebrew.platforms.linux_arm,
        ),
        (
            "linux_intel",
            "x86_64",
            "linux",
            homebrew.platforms.linux_intel,
        ),
    ]
}

fn homebrew_archive(homebrew: &ResolvedHomebrew, arch: &str, os: &str) -> String {
    homebrew
        .archive_pattern
        .replace("{name}", &homebrew.name)
        .replace("{version}", "${VERSION}")
        .replace("{arch}", arch)
        .replace("{os}", os)
}

fn scoop_windows_zip(scoop: &ResolvedScoop) -> String {
    scoop
        .archive_pattern
        .replace("{name}", &scoop.name)
        .replace("{version}", "${VERSION}")
        .replace("{arch}", "x86_64")
}

fn winget_manifest_dir(package_id: &str) -> String {
    let mut parts = package_id.split('.').collect::<Vec<_>>();
    if parts.is_empty() {
        return "manifests".to_owned();
    }
    let first = parts[0]
        .chars()
        .next()
        .map(|ch| ch.to_ascii_lowercase())
        .unwrap_or('x');
    let mut segments = vec!["manifests".to_owned(), first.to_string()];
    segments.extend(parts.drain(..).map(str::to_owned));
    segments.join("/")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn nix_path_info_arg(link: &str) -> String {
    if link.starts_with('/') || link.starts_with("./") || link.starts_with("../") {
        link.to_owned()
    } else {
        format!("./{link}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AurFlavors, ChocolateyPushConfig, HomebrewPlatformsConfig, ReleaseProvider, ScoopArchSet,
    };

    #[test]
    fn forgejo_action_references_are_immutable_and_versioned() {
        assert_eq!(
            forgejo_action_ref("checkout", "v4.3.1"),
            "https://code.forgejo.org/actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1"
        );
        assert_eq!(
            forgejo_action_ref("cache", "v4.3.0"),
            "https://code.forgejo.org/actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0"
        );
    }

    fn chocolatey() -> ResolvedChocolatey {
        ResolvedChocolatey {
            name: "modde".to_owned(),
            id: "modde".to_owned(),
            title: "modde".to_owned(),
            authors: Some("Caniko".to_owned()),
            description: "Cross-platform game mod manager".to_owned(),
            summary: None,
            project_url: "https://modde.rs".to_owned(),
            license_url: None,
            icon_url: None,
            package_source_url: None,
            docs_url: None,
            bug_tracker_url: None,
            project_source_url: None,
            tags: None,
            release_notes_url: None,
            download_repo: "caniko/rs-modde".to_owned(),
            archive_pattern: "modde-{version}-{arch}-windows.zip".to_owned(),
            push: ChocolateyPushConfig::default(),
            api_key_env: "CHOCOLATEY_API_KEY".to_owned(),
            api_key_secret: "chocolatey_api_key".to_owned(),
            api_key_from_runner: false,
            nix_tool: "github:caniko/nixpkgs/add-chocolatey-scoop#chocolatey git+https://codeberg.org/caniko/simit"
                .to_owned(),
        }
    }

    fn windows_signing() -> WindowsSigningConfig {
        WindowsSigningConfig {
            dir: "release/windows-x86_64".to_owned(),
            binaries: vec!["modde".to_owned(), "modde-ui".to_owned()],
            sign_name: "modde".to_owned(),
            sign_url: "https://modde.tartanoglu.com".to_owned(),
            timestamp_url: "http://timestamp.digicert.com".to_owned(),
            tar_archive: "modde-{version}-x86_64-windows.tar.gz".to_owned(),
            zip_archive: "modde-{version}-x86_64-windows.zip".to_owned(),
            pfx_secret: "WINDOWS_SIGNING_PFX".to_owned(),
            pass_secret: "WINDOWS_SIGNING_PASS".to_owned(),
            subject_secret: "WINDOWS_SIGNING_SUBJECT".to_owned(),
        }
    }

    fn flatpak() -> FlatpakConfig {
        FlatpakConfig {
            repo: "flathub/com.tartanoglu.modde".to_owned(),
            app_id: "com.tartanoglu.modde".to_owned(),
            manifest_files: vec![
                "com.tartanoglu.modde.json".to_owned(),
                "cargo-sources.json".to_owned(),
            ],
            base_branch: "master".to_owned(),
            token_secret: "FLATHUB_TOKEN".to_owned(),
        }
    }

    fn winget() -> WingetConfig {
        WingetConfig {
            package_id: "Caniko.Modde".to_owned(),
            download_repo: "caniko/rs-modde".to_owned(),
            zip_archive: "modde-{version}-x86_64-windows.zip".to_owned(),
            token_secret: "WINGET_PAT".to_owned(),
        }
    }

    fn artifacts() -> ArtifactsConfig {
        ArtifactsConfig {
            prebuild_binaries: false,
            runner: Some("atlas".to_owned()),
            substituters: vec!["https://cache.example/c".to_owned()],
            trusted_public_keys: vec!["c:abc=".to_owned()],
            version_attr: Some("modde".to_owned()),
            supply_chain_command: Some("cargo deny check".to_owned()),
            build_commands: vec!["nix build .#modde --out-link release/x".to_owned()],
            nix_bundle_attrs: vec![],
            sbom_commands: vec![],
            checksum_globs: vec!["*.tar.gz".to_owned(), "*.deb".to_owned()],
            minisign_pub: "keys/minisign.pub".to_owned(),
            sign: true,
        }
    }

    fn aur() -> ResolvedAur {
        ResolvedAur {
            name: "modde".to_owned(),
            description: "d".to_owned(),
            url: "https://codeberg.org/caniko/rs-modde".to_owned(),
            license: "GPL-3.0-only".to_owned(),
            maintainer: None,
            maintainer_gpg: None,
            arch: "x86_64".to_owned(),
            depends: vec![],
            makedepends: vec![],
            bin_glibc_min: None,
            binaries: vec!["modde".to_owned()],
            assets: vec![],
            license_file: None,
            readme: None,
            changelog: None,
            download_repo: "caniko/rs-modde".to_owned(),
            repo: "rs-modde".to_owned(),
            source_archive_pattern: "{repo}-{version}.tar.gz".to_owned(),
            binary_archive_pattern: "{name}-{version}-x86_64-linux.tar.gz".to_owned(),
            git_url: "https://codeberg.org/caniko/rs-modde.git".to_owned(),
            flavors: AurFlavors::default(),
            ssh_remote: "ssh://aur@aur.archlinux.org".to_owned(),
            ssh_key_secret: "AUR_SSH_KEY".to_owned(),
            stable_only: true,
        }
    }

    fn copr() -> ResolvedCopr {
        ResolvedCopr {
            name: "modde".to_owned(),
            summary: "s".to_owned(),
            description: "d".to_owned(),
            license: "GPL-3.0-only".to_owned(),
            url: "https://codeberg.org/caniko/rs-modde".to_owned(),
            download_repo: "caniko/rs-modde".to_owned(),
            repo: "rs-modde".to_owned(),
            build_requires: vec![],
            binaries: vec!["modde".to_owned()],
            spec_path: "modde.spec".to_owned(),
            project: Some("caniko/rs-modde".to_owned()),
            testing_project: Some("caniko/rs-modde-testing".to_owned()),
            login_secret: "copr_login".to_owned(),
            username_secret: "copr_username".to_owned(),
            token_secret: "copr_token".to_owned(),
            nix_tool: ".#copr-cli".to_owned(),
        }
    }

    fn apt() -> ResolvedApt {
        ResolvedApt {
            label: "modde".to_owned(),
            repo_url: "ssh://git@codeberg.org/caniko/rs-modde-apt.git".to_owned(),
            branch: "pages".to_owned(),
            distribution: "stable".to_owned(),
            architectures: "amd64".to_owned(),
            components: "main".to_owned(),
            debian_release: "bookworm".to_owned(),
            packages: vec!["modde-cli=modde".to_owned()],
            build_deps: vec!["gcc".to_owned(), "pkg-config".to_owned()],
            cargo_deb_version: "2.5.0".to_owned(),
            gpg_key_secret: "modde_apt_repo_gpg_key".to_owned(),
            gpg_key_id_secret: "modde_apt_repo_gpg_key_id".to_owned(),
            gpg_passphrase_secret: "modde_apt_repo_gpg_passphrase".to_owned(),
            ssh_key_secret: "modde_apt_repo_ssh_key".to_owned(),
        }
    }

    fn codeberg() -> ResolvedReleaseTarget {
        ResolvedReleaseTarget {
            provider: ReleaseProvider::Forgejo,
            repo: "caniko/rs-modde".to_owned(),
            api_base: "https://codeberg.org/api/v1".to_owned(),
            token_secret: "codeberg_token".to_owned(),
            target_branch: "trunk".to_owned(),
            body_from_changelog: true,
        }
    }

    fn homebrew() -> ResolvedHomebrew {
        ResolvedHomebrew {
            name: "modde".to_owned(),
            binaries: vec!["modde".to_owned(), "modde-ui".to_owned()],
            tap_url: "https://codeberg.org/caniko/homebrew-modde.git".to_owned(),
            tap_token_secret: "FORGEJO_HOMEBREW_TOKEN".to_owned(),
            description: "Cross-platform game mod manager".to_owned(),
            homepage: "https://modde.rs".to_owned(),
            license: "GPL-3.0-only".to_owned(),
            download_repo: "caniko/rs-modde".to_owned(),
            archive_pattern: "modde-{version}-{arch}-{os}.tar.gz".to_owned(),
            platforms: HomebrewPlatformsConfig::default(),
        }
    }

    fn scoop() -> ResolvedScoop {
        ResolvedScoop {
            name: "modde".to_owned(),
            bucket_url: "https://codeberg.org/caniko/scoop-modde.git".to_owned(),
            bucket_token_secret: "FORGEJO_SCOOP_TOKEN".to_owned(),
            description: "d".to_owned(),
            homepage: "https://modde.rs".to_owned(),
            license: "GPL-3.0-only".to_owned(),
            download_repo: "caniko/rs-modde".to_owned(),
            archive_pattern: "modde-{version}-{arch}-windows.zip".to_owned(),
            binaries: vec!["modde".to_owned()],
            architectures: ScoopArchSet::default(),
        }
    }

    #[test]
    fn full_pipeline_has_every_channel_mechanic() {
        let (aur, copr, apt, codeberg, homebrew, scoop) =
            (aur(), copr(), apt(), codeberg(), homebrew(), scoop());
        let (chocolatey, windows, flatpak, winget) =
            (chocolatey(), windows_signing(), flatpak(), winget());
        let announce = AnnounceConfig::default();
        let mut artifacts = artifacts();
        artifacts.sbom_commands = vec!["cargo sbom > release/sbom.json".to_owned()];
        let workflow = render(&ReleaseWorkflowInputs {
            platform: Platform::Forgejo,
            runner: "atlas",
            preinstalled_nix: false,
            publish_enforcement: ReleasePublisherEnforcement::Declared,
            artifacts: &artifacts,
            smoke_command: Some("nix run .#release-smoke --"),
            release: Some(&codeberg),
            attic: None,
            aur: Some(&aur),
            copr: Some(&copr),
            apt: Some(&apt),
            homebrew: Some(&homebrew),
            scoop: Some(&scoop),
            chocolatey: Some(&chocolatey),
            windows_signing: Some(&windows),
            flatpak: Some(&flatpak),
            winget: Some(&winget),
            announce: Some(&announce),
        });

        // Scaffolding
        assert!(workflow.contains("name: release\n"));
        assert!(workflow.contains("runs-on: atlas\n"));
        assert!(workflow.contains("enable-openid-connect: true\n"));
        assert!(
            workflow.contains(
                "VERSION=\"${GITHUB_REF_NAME:-${FORGE_REF_NAME:-${CODEBERG_REF_NAME:-}}}\""
            )
        );
        assert!(!workflow.contains("inputs.version"));
        assert!(
            workflow.contains(
                "test \"$(nix eval --raw \"$tag_worktree#modde.version\")\" = \"$VERSION\""
            )
        );
        assert!(workflow.contains("git worktree add --detach \"$tag_worktree\" \"$VERSION\""));
        assert!(workflow.contains("git verify-tag \"$VERSION\""));
        assert!(workflow.contains("git checkout --detach \"$validated_sha\""));
        assert!(workflow.contains("export VERSION IS_PRERELEASE"));
        assert!(workflow.contains("nix run .#release-smoke -- \"$VERSION\" release"));
        // minisign + cosign SLSA attestation
        assert!(workflow.contains("minisign -S -s \"$minisign_key\""));
        assert!(workflow.contains("cosign sign-blob --yes --identity-token"));
        assert!(workflow.contains("--type slsaprovenance1"));
        assert!(workflow.contains("continuing without cosign signature or attestation for $file"));
        // Codeberg release upload
        assert!(workflow.contains("CODEBERG_TOKEN: ${{ secrets.codeberg_token }}"));
        assert!(workflow.contains(
            "require_credential 'global/user secret' 'codeberg_token' \"${CODEBERG_TOKEN:-}\""
        ));
        assert!(workflow.contains("Missing release credentials:%s\\n"));
        assert!(workflow.contains("${CODEBERG_API}/repos/${CODEBERG_REPO}/releases\""));
        assert!(workflow.contains("releases/${release_id}/assets?name=${name}"));
        assert!(workflow.contains("target_commitish: $branch"));
        // COPR srpm build + push
        assert!(workflow.contains("rpmbuild -bs modde.spec"));
        assert!(workflow.contains("cargo vendor vendor > ../cargo-vendor-config.toml"));
        assert!(
            workflow.contains(
                "nix run .#copr-cli -- build --nowait \"${COPR_PROJECT}\" srpms/*.src.rpm"
            )
        );
        assert!(workflow.contains("COPR_PROJECT=\"caniko/rs-modde\""));
        assert!(workflow.contains("COPR_PROJECT=\"caniko/rs-modde-testing\""));
        assert!(workflow.contains("COPR_USERNAME: ${{ vars.copr_username }}"));
        assert!(!workflow.contains("COPR_USERNAME: ${{ secrets.copr_username }}"));
        assert!(
            workflow.contains(
                "require_credential 'global/user secret' 'copr_login' \"${COPR_LOGIN:-}\""
            )
        );
        assert!(workflow.contains(
            "require_credential 'global/user variable' 'copr_username' \"${COPR_USERNAME:-}\""
        ));
        assert!(
            workflow.contains(
                "require_credential 'global/user secret' 'copr_token' \"${COPR_TOKEN:-}\""
            )
        );
        // APT deb build + reprepro publish
        assert!(workflow.contains("nix develop -c bash scripts/build-deb.sh \"$VERSION\" release"));
        assert!(workflow.contains("continuing without .deb release assets and APT publish."));
        assert!(!workflow.contains("debootstrap --variant=minbase"));
        assert!(!workflow.contains("nixpkgs#debootstrap"));
        assert!(!workflow.contains("priv chroot"));
        assert!(!workflow.contains("sudo chroot"));
        assert!(workflow.contains("reprepro -b \"$work/apt\" includedeb \"$APT_DISTRIBUTION\""));
        assert!(workflow.contains("APT_REPO_GPG_KEY_ID: ${{ vars.modde_apt_repo_gpg_key_id }}"));
        assert!(
            !workflow.contains("APT_REPO_GPG_KEY_ID: ${{ secrets.modde_apt_repo_gpg_key_id }}")
        );
        assert!(
            workflow
                .contains("APT_REPO_GPG_FINGERPRINT: ${{ vars.modde_apt_repo_gpg_fingerprint }}")
        );
        assert!(
            workflow.contains("APT_REPO_GPG_PUBLIC_KEY: ${{ vars.modde_apt_repo_gpg_public_key }}")
        );
        assert!(workflow.contains("require_credential 'repo secret'"));
        assert!(workflow.contains("\"${APT_REPO_GPG_KEY:-}\""));
        assert!(workflow.contains("require_credential 'repo variable'"));
        assert!(workflow.contains("\"${MODDE_APT_REPO_GPG_KEY_ID:-}\""));
        assert!(workflow.contains("\"${MODDE_APT_REPO_GPG_FINGERPRINT:-}\""));
        assert!(workflow.contains("\"${MODDE_APT_REPO_GPG_PUBLIC_KEY:-}\""));
        assert!(
            workflow.contains(
                "git push --force-with-lease origin \"HEAD:refs/heads/${APT_REPO_BRANCH}\""
            )
        );
        // AUR ssh publish with .SRCINFO + 3 flavors
        assert!(workflow.contains("AUR_SSH_KEY: ${{ secrets.AUR_SSH_KEY }}"));
        assert!(workflow.contains(
            "require_credential 'global/user secret' 'AUR_SSH_KEY' \"${AUR_SSH_KEY:-}\""
        ));
        assert!(workflow.contains("makepkg --config \"$MAKEPKG_CONF\" --printsrcinfo > .SRCINFO"));
        assert!(
            workflow
                .contains("for pkg in modde modde-bin modde-git; do publish_pkg \"$pkg\"; done")
        );
        // Homebrew via rs-harbor, Scoop via sed template
        assert!(workflow.contains("HOMEBREW_TAP_TOKEN: ${{ secrets.FORGEJO_HOMEBREW_TOKEN }}"));
        assert!(workflow.contains("SCOOP_BUCKET_TOKEN: ${{ secrets.FORGEJO_SCOOP_TOKEN }}"));
        assert!(workflow.contains("nix run '.#rs-harbor' -- brew bump"));
        assert!(workflow.contains("dist/scoop/modde.json > scoop-bucket/bucket/modde.json"));
        // Prerelease gating present on downstream package repos
        assert!(workflow.contains("skipping AUR packages."));
        assert!(workflow.contains("skipping Homebrew tap update."));
        assert!(workflow.contains(
            "HOMEBREW_TAP_TOKEN is required because Homebrew tap publishing is configured."
        ));
        assert!(workflow.contains(
            "SCOOP_BUCKET_TOKEN is required because Scoop bucket publishing is configured."
        ));
        assert!(
            workflow
                .contains("FLATHUB_TOKEN is required because Flathub publishing is configured.")
        );
        assert!(
            workflow.contains("WINGET_PAT is required because WinGet publishing is configured.")
        );
        assert!(workflow.contains(
            "CHOCOLATEY_API_KEY is required because Chocolatey package publishing is configured."
        ));
        assert!(workflow.contains(
            "Mastodon secrets are required because release announcements are configured."
        ));
        assert!(
            workflow.contains(
                "Matrix secrets are required because release announcements are configured."
            )
        );
        assert!(!workflow.contains("apt secrets unset; skipping."));
        assert!(!workflow.contains("COPR credentials unset; skipping."));
        assert!(!workflow.contains("HOMEBREW_TAP_TOKEN unset; skipping."));
        assert!(!workflow.contains("SCOOP_BUCKET_TOKEN unset; skipping."));
        assert!(!workflow.contains("FLATHUB_TOKEN unset; skipping."));
        assert!(!workflow.contains("WINGET_PAT unset; skipping."));
        assert!(!workflow.contains("CHOCOLATEY_API_KEY unset; skipping."));
        assert!(!workflow.contains("Mastodon secrets unset; skipping."));
        assert!(!workflow.contains("Matrix secrets unset; skipping."));
        // Stage B channels
        assert!(workflow.contains("cargo sbom > release/sbom.json"));
        assert!(workflow.contains("osslsigncode sign -pkcs12"));
        assert!(workflow.contains("zip_out=\"$PWD/release/modde-${VERSION}-x86_64-windows.zip\""));
        assert!(!workflow.contains("OLDPWD"));
        assert!(workflow.contains("require_credential 'global/user secret'"));
        assert!(workflow.contains("\"${CHOCOLATEY_API_KEY:-}\""));
        assert!(workflow.contains("https://github.com/flathub/com.tartanoglu.modde.git"));
        assert!(workflow.contains("wine \"$tmpdir/wingetcreate.exe\" update Caniko.Modde"));
        assert!(workflow.contains("/api/v1/statuses"));
        assert!(workflow.contains(
            "nix shell github:caniko/nixpkgs/add-chocolatey-scoop#chocolatey git+https://codeberg.org/caniko/simit -c simit dist chocolatey bump"
        ));
        // Checksum step uses runner-compatible Bash glob enumeration, not find/xargs.
        assert!(workflow.contains("add_matches '*.tar.gz'"));
        assert!(workflow.contains("add_matches '*.deb'"));
        assert!(workflow.contains("add_matches 'release/SHA256SUMS.txt'"));
        assert!(workflow.contains("add_matches 'release/*.tar.gz'"));
        assert!(!workflow.contains("files=(release/*)"));
        assert!(workflow.contains("sha256sum \"$file\" >> SHA256SUMS.txt"));
        assert!(workflow.contains("test -s SHA256SUMS.txt"));
        assert!(workflow.contains("test -s SHA256SUMS.txt\n      - name: Sign checksums"));
        // Order: codeberg before downstream, copr near the end
        let pos = |needle: &str| workflow.find(needle).unwrap();
        assert!(pos("Build Debian packages") < pos("Publish Codeberg release"));
        assert!(pos("Publish Codeberg release") < pos("Publish APT repository"));
        assert!(pos("Publish AUR packages") < pos("Publish Homebrew tap"));
        assert!(pos("Build SRPM for COPR") < pos("Build release artifacts"));
    }

    #[test]
    fn activated_remote_policy_probes_and_hardens_active_publishers() {
        let (aur, copr, apt, codeberg, homebrew, scoop) =
            (aur(), copr(), apt(), codeberg(), homebrew(), scoop());
        let (chocolatey, flatpak, winget) = (chocolatey(), flatpak(), winget());
        let artifacts = artifacts();
        let workflow = render(&ReleaseWorkflowInputs {
            platform: Platform::Forgejo,
            runner: "atlas",
            preinstalled_nix: false,
            publish_enforcement: ReleasePublisherEnforcement::ActivatedRemote,
            artifacts: &artifacts,
            smoke_command: None,
            release: Some(&codeberg),
            attic: None,
            aur: Some(&aur),
            copr: Some(&copr),
            apt: Some(&apt),
            homebrew: Some(&homebrew),
            scoop: Some(&scoop),
            chocolatey: Some(&chocolatey),
            windows_signing: None,
            flatpak: Some(&flatpak),
            winget: Some(&winget),
            announce: None,
        });

        assert!(workflow.contains("      - name: Probe downstream publisher state\n"));
        assert!(workflow.contains(": > release-publisher-state.env"));
        assert!(workflow.contains("write_state \"$channel\" active-required \"$detail\""));
        assert!(workflow.contains("write_state \"$channel\" inactive-soft \"$detail\""));
        assert!(workflow.contains("write_state \"$channel\" skipped-prerelease \"$detail\""));
        assert!(workflow.contains("stable_or_state apt"));
        assert!(workflow.contains("active_or_soft copr \"$COPR_PROBE_PROJECT\""));
        assert!(workflow.contains("stable_or_state chocolatey"));
        assert!(workflow.contains("stable_or_state homebrew"));
        assert!(workflow.contains("stable_or_state scoop"));
        assert!(workflow.contains("stable_or_state flathub"));
        assert!(workflow.contains("manifests/c/Caniko/Modde"));
        assert!(workflow.contains("publisher_missing apt 'apt secrets unset'"));
        assert!(workflow.contains("publisher_missing aur 'AUR_SSH_KEY unset'"));
        assert!(workflow.contains("publisher_missing copr 'COPR credentials unset'"));
        assert!(workflow.contains("publisher_missing chocolatey 'CHOCOLATEY_API_KEY unset'"));
        assert!(workflow.contains("publisher_missing homebrew 'HOMEBREW_TAP_TOKEN unset'"));
        assert!(workflow.contains("publisher_missing scoop 'SCOOP_BUCKET_TOKEN unset'"));
        assert!(workflow.contains("publisher_missing flathub 'FLATHUB_TOKEN unset'"));
        assert!(workflow.contains("publisher_missing winget 'WINGET_PAT unset'"));
        assert!(
            workflow.contains(
                "configured publisher cannot run without required credentials/artifacts."
            )
        );
        assert!(!workflow.contains("bootstrap channel is inactive, skipping."));
        assert!(!workflow.contains("require_credential 'repo secret' 'modde_apt_repo_gpg_key'"));
        assert!(!workflow.contains("require_credential 'global/user secret' 'AUR_SSH_KEY'"));
        assert!(!workflow.contains("require_credential 'global/user secret' 'copr_login'"));
    }

    #[test]
    fn nix_bundle_attributes_are_built_and_manifest_checked() {
        let mut artifacts = artifacts();
        artifacts.nix_bundle_attrs = vec!["release-bundle".to_owned()];
        let workflow = render(&ReleaseWorkflowInputs {
            platform: Platform::Forgejo,
            runner: "atlas",
            preinstalled_nix: false,
            publish_enforcement: ReleasePublisherEnforcement::Declared,
            artifacts: &artifacts,
            smoke_command: None,
            release: None,
            attic: None,
            aur: None,
            copr: None,
            apt: None,
            homebrew: None,
            scoop: None,
            chocolatey: None,
            windows_signing: None,
            flatpak: None,
            winget: None,
            announce: None,
        });
        assert!(workflow.contains("nix build '.#release-bundle'"));
        assert!(workflow.contains("expected exactly one release manifest from Nix bundles"));
        assert!(workflow.contains(".schemaVersion == 2"));
    }

    #[test]
    fn prebuild_toggle_uses_conventional_bundle_and_explicit_attrs_win() {
        let mut artifacts = artifacts();
        artifacts.prebuild_binaries = true;
        assert_eq!(
            artifacts.effective_nix_bundle_attrs(),
            vec!["release-bundle".to_owned()]
        );

        artifacts.nix_bundle_attrs = vec!["packages.x86_64-linux.release".to_owned()];
        assert_eq!(
            artifacts.effective_nix_bundle_attrs(),
            vec!["packages.x86_64-linux.release".to_owned()]
        );
    }

    #[test]
    fn checksum_step_handles_empty_globs() {
        let mut artifacts = artifacts();
        artifacts.checksum_globs = vec![];
        let workflow = render(&ReleaseWorkflowInputs {
            platform: Platform::Forgejo,
            runner: "atlas",
            preinstalled_nix: false,
            publish_enforcement: ReleasePublisherEnforcement::Declared,
            artifacts: &artifacts,
            smoke_command: None,
            release: None,
            attic: None,
            aur: None,
            copr: None,
            apt: None,
            homebrew: None,
            scoop: None,
            chocolatey: None,
            windows_signing: None,
            flatpak: None,
            winget: None,
            announce: None,
        });
        // Empty checksum_globs defaults to hashing all release directory files.
        assert!(workflow.contains("add_matches '*'"));
        assert!(workflow.contains("sha256sum \"$file\" >> SHA256SUMS.txt"));
        assert!(workflow.contains("test -s SHA256SUMS.txt"));
        // No bare `sha256sum *` fallback
        assert!(!workflow.contains("sha256sum *"));
    }

    #[test]
    fn checksum_step_does_not_contain_bare_required_deb_glob() {
        let (aur, copr, codeberg) = (aur(), copr(), codeberg());
        let artifacts = artifacts();
        // The test fixture has checksum_globs = ["*.tar.gz", "*.deb"]
        let workflow = render(&ReleaseWorkflowInputs {
            platform: Platform::Forgejo,
            runner: "atlas",
            preinstalled_nix: false,
            publish_enforcement: ReleasePublisherEnforcement::Declared,
            artifacts: &artifacts,
            smoke_command: Some("nix run .#release-smoke --"),
            release: Some(&codeberg),
            attic: None,
            aur: Some(&aur),
            copr: Some(&copr),
            apt: None,
            homebrew: None,
            scoop: None,
            chocolatey: None,
            windows_signing: None,
            flatpak: None,
            winget: None,
            announce: None,
        });
        // The generated checksum command uses Bash glob enumeration, not bare `sha256sum *.deb`.
        assert!(
            !workflow.contains("sha256sum *.deb"),
            "checksums must not hard-require *.deb files"
        );
        assert!(
            !workflow.contains("sha256sum *.tar.gz"),
            "checksums must not use bare globs"
        );
        assert!(
            !workflow.contains("xargs -0 sha256sum"),
            "runner image does not provide xargs"
        );
    }
}
