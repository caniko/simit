//! Render Arch `PKGBUILD` files for the source / `-bin` / `-git` AUR flavors.
//!
//! `${pkgver}` is kept as a literal bash expansion in `source=` URLs (makepkg
//! substitutes it); only the `pkgver=` line carries the rendered version. The
//! release workflow rewrites `pkgver`/`sha256sums` per release via `simit dist
//! aur bump`.

use std::fmt::Write as _;

use crate::config::ResolvedAur;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Flavor {
    Source,
    Bin,
    Git,
}

impl Flavor {
    /// The AUR `pkgname` / directory name for this flavor.
    pub fn pkgname(self, base: &str) -> String {
        match self {
            Self::Source => base.to_owned(),
            Self::Bin => format!("{base}-bin"),
            Self::Git => format!("{base}-git"),
        }
    }
}

/// A rendered PKGBUILD keyed by its AUR package directory name.
#[derive(Debug)]
pub struct RenderedPkgbuild {
    pub pkgname: String,
    pub flavor: Flavor,
    pub pkgbuild: String,
}

/// Render every enabled flavor for the given release version.
pub fn render(aur: &ResolvedAur, version: &str) -> Vec<RenderedPkgbuild> {
    let mut out = Vec::new();
    if aur.flavors.source {
        out.push(render_flavor(aur, version, Flavor::Source));
    }
    if aur.flavors.bin {
        out.push(render_flavor(aur, version, Flavor::Bin));
    }
    if aur.flavors.git {
        out.push(render_flavor(aur, version, Flavor::Git));
    }
    out
}

pub fn render_flavor(aur: &ResolvedAur, version: &str, flavor: Flavor) -> RenderedPkgbuild {
    let pkgname = flavor.pkgname(&aur.name);
    let mut out = String::new();

    if let Some(maintainer) = &aur.maintainer {
        writeln!(out, "# Maintainer: {maintainer}").expect("write");
    }
    if let Some(fingerprint) = &aur.maintainer_gpg {
        writeln!(out, "# Maintainer GPG key: {fingerprint}").expect("write");
    }

    writeln!(out, "pkgname={pkgname}").expect("write");
    writeln!(out, "pkgver={version}").expect("write");
    out.push_str("pkgrel=1\n");
    writeln!(out, "pkgdesc={}", single_quote(&aur.description)).expect("write");
    writeln!(out, "arch=('{}')", aur.arch).expect("write");
    writeln!(out, "url={}", single_quote(&aur.url)).expect("write");
    writeln!(
        out,
        "license=({})",
        quoted_array(std::slice::from_ref(&aur.license))
    )
    .expect("write");
    writeln!(out, "depends=({})", quoted_array(&depends_for(aur, flavor))).expect("write");
    let makedepends = makedepends_for(aur, flavor);
    if !makedepends.is_empty() {
        writeln!(out, "makedepends=({})", quoted_array(&makedepends)).expect("write");
    }
    if flavor != Flavor::Source {
        writeln!(
            out,
            "provides=({})",
            quoted_array(std::slice::from_ref(&aur.name))
        )
        .expect("write");
    }
    writeln!(
        out,
        "conflicts=({})",
        quoted_array(&conflicts_for(aur, flavor))
    )
    .expect("write");

    push_source_and_sums(&mut out, aur, flavor);

    if flavor == Flavor::Git {
        push_pkgver_function(&mut out, &aur.repo);
    }
    if flavor != Flavor::Bin {
        push_build_function(&mut out, aur);
    }
    push_package_function(&mut out, aur, flavor);

    RenderedPkgbuild {
        pkgname,
        flavor,
        pkgbuild: out,
    }
}

fn depends_for(aur: &ResolvedAur, flavor: Flavor) -> Vec<String> {
    let mut depends = aur.depends.clone();
    if flavor == Flavor::Bin {
        if let Some(min) = &aur.bin_glibc_min {
            for entry in &mut depends {
                if entry == "glibc" {
                    *entry = format!("glibc>={min}");
                }
            }
        }
    }
    depends
}

fn makedepends_for(aur: &ResolvedAur, flavor: Flavor) -> Vec<String> {
    match flavor {
        Flavor::Source => aur.makedepends.clone(),
        Flavor::Git => {
            let mut deps = aur.makedepends.clone();
            if !deps.iter().any(|dep| dep == "git") {
                deps.push("git".to_owned());
            }
            deps.sort();
            deps
        }
        Flavor::Bin => vec!["patchelf".to_owned()],
    }
}

fn conflicts_for(aur: &ResolvedAur, flavor: Flavor) -> Vec<String> {
    [Flavor::Source, Flavor::Bin, Flavor::Git]
        .into_iter()
        .filter(|other| *other != flavor)
        .map(|other| other.pkgname(&aur.name))
        .collect()
}

fn push_source_and_sums(out: &mut String, aur: &ResolvedAur, flavor: Flavor) {
    match flavor {
        Flavor::Source => {
            let archive = source_archive_name(aur);
            writeln!(
                out,
                "source=(\"{archive}::{base}/{archive}\")",
                base = release_download_base(aur),
            )
            .expect("write");
            out.push_str("sha256sums=('SKIP')\n");
        }
        Flavor::Bin => {
            let bin_archive = binary_archive_name(aur);
            let source_archive = source_archive_name(aur);
            let base = release_download_base(aur);
            out.push_str("source=(\n");
            writeln!(out, "  \"{bin_archive}::{base}/{bin_archive}\"").expect("write");
            writeln!(out, "  \"{source_archive}::{base}/{source_archive}\"").expect("write");
            out.push_str(")\n");
            out.push_str("sha256sums=('SKIP'\n            'SKIP')\n");
        }
        Flavor::Git => {
            writeln!(out, "source=('git+{}')", aur.git_url).expect("write");
            out.push_str("sha256sums=('SKIP')\n");
        }
    }
}

fn push_pkgver_function(out: &mut String, repo: &str) {
    out.push('\n');
    out.push_str("pkgver() {\n");
    writeln!(out, "  cd {repo}").expect("write");
    out.push_str(
        "  git describe --long --tags 2>/dev/null | sed 's/^v//;s/\\([^-]*-g\\)/r\\1/;s/-/./g' || echo \"$pkgver\"\n",
    );
    out.push_str("}\n");
}

fn push_build_function(out: &mut String, aur: &ResolvedAur) {
    out.push('\n');
    out.push_str("build() {\n");
    writeln!(out, "  cd {}", aur.repo).expect("write");
    out.push_str("  cargo build --release --locked");
    for binary in &aur.binaries {
        write!(out, " --bin {binary}").expect("write");
    }
    out.push('\n');
    out.push_str("}\n");
}

fn push_package_function(out: &mut String, aur: &ResolvedAur, flavor: Flavor) {
    out.push('\n');
    out.push_str("package() {\n");

    // Source prefix for non-binary install inputs: source/git package() runs
    // after `cd <repo>`, while -bin installs prebuilt binaries from the archive
    // root and pulls assets from the extracted source tree under <repo>/.
    let asset_prefix = match flavor {
        Flavor::Source | Flavor::Git => {
            writeln!(out, "  cd {}", aur.repo).expect("write");
            String::new()
        }
        Flavor::Bin => format!("{}/", aur.repo),
    };

    match flavor {
        Flavor::Source | Flavor::Git => {
            for binary in &aur.binaries {
                writeln!(
                    out,
                    "  install -Dm755 target/release/{binary} \"$pkgdir/usr/bin/{binary}\""
                )
                .expect("write");
            }
        }
        Flavor::Bin => {
            for binary in &aur.binaries {
                writeln!(
                    out,
                    "  install -Dm755 {binary} \"$pkgdir/usr/bin/{binary}\""
                )
                .expect("write");
            }
            out.push('\n');
            out.push_str("  for binary in ");
            let targets = aur
                .binaries
                .iter()
                .map(|binary| format!("\"$pkgdir/usr/bin/{binary}\""))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&targets);
            out.push_str("; do\n");
            out.push_str("    patchelf \\\n");
            out.push_str("      --set-interpreter /usr/lib/ld-linux-x86-64.so.2 \\\n");
            out.push_str("      --set-rpath /usr/lib \\\n");
            out.push_str("      \"$binary\"\n");
            out.push_str("  done\n");
            if !aur.assets.is_empty()
                || aur.license_file.is_some()
                || aur.readme.is_some()
                || aur.changelog.is_some()
            {
                out.push('\n');
            }
        }
    }

    for asset in &aur.assets {
        writeln!(
            out,
            "  install -Dm{mode} {prefix}{source} \"$pkgdir/{dest}\"",
            mode = asset.mode,
            prefix = asset_prefix,
            source = asset.source,
            dest = asset.dest,
        )
        .expect("write");
    }
    if let Some(license) = &aur.license_file {
        writeln!(
            out,
            "  install -Dm644 {asset_prefix}{license} \"$pkgdir/usr/share/licenses/$pkgname/{license}\""
        )
        .expect("write");
    }
    if let Some(readme) = &aur.readme {
        writeln!(
            out,
            "  install -Dm644 {asset_prefix}{readme} \"$pkgdir/usr/share/doc/$pkgname/{readme}\""
        )
        .expect("write");
    }
    if let Some(changelog) = &aur.changelog {
        writeln!(
            out,
            "  install -Dm644 {asset_prefix}{changelog} \"$pkgdir/usr/share/doc/$pkgname/{changelog}\""
        )
        .expect("write");
    }

    out.push_str("}\n");
}

/// Release-asset download base, e.g. `https://codeberg.org/owner/repo/releases/download/${pkgver}`.
pub fn release_download_base(aur: &ResolvedAur) -> String {
    format!(
        "https://codeberg.org/{}/releases/download/${{pkgver}}",
        aur.download_repo
    )
}

/// Source tarball file name with `${pkgver}` kept literal.
pub fn source_archive_name(aur: &ResolvedAur) -> String {
    aur.source_archive_pattern
        .replace("{repo}", &aur.repo)
        .replace("{name}", &aur.name)
        .replace("{version}", "${pkgver}")
}

/// Prebuilt-binary archive file name with `${pkgver}` kept literal.
pub fn binary_archive_name(aur: &ResolvedAur) -> String {
    aur.binary_archive_pattern
        .replace("{repo}", &aur.repo)
        .replace("{name}", &aur.name)
        .replace("{version}", "${pkgver}")
}

fn quoted_array(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("'{value}'"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AurFlavors;

    fn sample() -> ResolvedAur {
        ResolvedAur {
            name: "modde".to_owned(),
            description: "Cross-platform game mod manager".to_owned(),
            url: "https://codeberg.org/caniko/rs-modde".to_owned(),
            license: "GPL-3.0-only".to_owned(),
            maintainer: Some("Can H. Tartanoglu <caniko@codeberg.org>".to_owned()),
            maintainer_gpg: Some("818D507F1E62139F8A17EAA64623DEA06FDACFE1".to_owned()),
            arch: "x86_64".to_owned(),
            depends: vec![
                "dbus".to_owned(),
                "gcc-libs".to_owned(),
                "glibc".to_owned(),
                "libxkbcommon".to_owned(),
                "openssl".to_owned(),
                "sqlite".to_owned(),
                "vulkan-icd-loader".to_owned(),
                "wayland".to_owned(),
            ],
            makedepends: vec![
                "cargo".to_owned(),
                "cmake".to_owned(),
                "pkgconf".to_owned(),
                "rust".to_owned(),
            ],
            bin_glibc_min: Some("2.38".to_owned()),
            binaries: vec!["modde".to_owned(), "modde-ui".to_owned()],
            assets: vec![],
            license_file: Some("LICENSE".to_owned()),
            readme: Some("README.md".to_owned()),
            changelog: Some("CHANGELOG.md".to_owned()),
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

    #[test]
    fn source_flavor() {
        let rendered = render_flavor(&sample(), "0.2.0", Flavor::Source);
        assert_eq!(rendered.pkgname, "modde");
        let body = rendered.pkgbuild;
        assert!(body.contains("pkgname=modde\n"));
        assert!(body.contains("pkgver=0.2.0\n"));
        assert!(body.contains("makedepends=('cargo' 'cmake' 'pkgconf' 'rust')\n"));
        assert!(body.contains("conflicts=('modde-bin' 'modde-git')\n"));
        assert!(body.contains("source=(\"rs-modde-${pkgver}.tar.gz::https://codeberg.org/caniko/rs-modde/releases/download/${pkgver}/rs-modde-${pkgver}.tar.gz\")\n"));
        assert!(body.contains("  cargo build --release --locked --bin modde --bin modde-ui\n"));
        assert!(!body.contains("provides="));
    }

    #[test]
    fn bin_flavor() {
        let rendered = render_flavor(&sample(), "0.2.0", Flavor::Bin);
        assert_eq!(rendered.pkgname, "modde-bin");
        let body = rendered.pkgbuild;
        assert!(body.contains("depends=('dbus' 'gcc-libs' 'glibc>=2.38'"));
        assert!(body.contains("makedepends=('patchelf')\n"));
        assert!(body.contains("provides=('modde')\n"));
        assert!(body.contains("conflicts=('modde' 'modde-git')\n"));
        assert!(body.contains("  \"modde-${pkgver}-x86_64-linux.tar.gz::"));
        assert!(body.contains("sha256sums=('SKIP'\n            'SKIP')\n"));
        assert!(body.contains("--set-interpreter /usr/lib/ld-linux-x86-64.so.2"));
        assert!(body.contains("install -Dm644 rs-modde/LICENSE"));
        assert!(!body.contains("build() {"));
    }

    #[test]
    fn git_flavor() {
        let rendered = render_flavor(&sample(), "0.2.0", Flavor::Git);
        assert_eq!(rendered.pkgname, "modde-git");
        let body = rendered.pkgbuild;
        assert!(body.contains("provides=('modde')\n"));
        assert!(body.contains("conflicts=('modde' 'modde-bin')\n"));
        assert!(body.contains("source=('git+https://codeberg.org/caniko/rs-modde.git')\n"));
        assert!(body.contains("pkgver() {\n"));
        assert!(body.contains("makedepends=('cargo' 'cmake' 'git' 'pkgconf' 'rust')\n"));
    }
}
