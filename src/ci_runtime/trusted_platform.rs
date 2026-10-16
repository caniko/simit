//! Fixed platform trust paths are independent of mutable job environment.
#[cfg(unix)]
pub(super) fn protected_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return false;
    };
    if !canonical.is_file() {
        return false;
    }
    [path, canonical.as_path()].into_iter().all(|path| {
        path.ancestors().all(|ancestor| {
            std::fs::symlink_metadata(ancestor).is_ok_and(|metadata| {
                metadata.uid() == 0
                    && (metadata.file_type().is_symlink()
                    || metadata.mode() & 0o022 == 0
                    // Nix's root-owned sticky store protects root-owned entries.
                    || (metadata.is_dir() && metadata.mode() & 0o1000 != 0))
            })
        })
    })
}
