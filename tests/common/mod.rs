use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

const MAINTAINERS_ASC: &str = r#"-----BEGIN PGP PUBLIC KEY BLOCK-----

mDMEaRBcoRYJKwYBBAHaRw8BAQdAfqwJgLlh5ZUmOlW3/xcBilGd881RdfZ59DwW
o6/v7/C0MkNhbiBILiBUYXJ0YW5vZ2x1IChjYW5pa28pIDxncGdAcm90YXMubW96
bWFpbC5jb20+iI4EExYKADYWIQSBjVB/HmITn4oX6qZGI96gb9rP4QUCaRBcoQIb
AwQLCQgHBBUKCQgFFgIDAQACHgECF4AACgkQRiPeoG/az+EaUgD/alnv6/d0AbBz
d20nszdrhtjZ8xxZjIbvVPK7vMos6N4A/3adtsq7fWaZo2gNmpvQVToatnTlyql4
CEbfssvyI6QEuDMEaRBcoRYJKwYBBAHaRw8BAQdAdunfzIjlOUTj2i1Xto03oDNc
NdovI6ECEDHjAYMRHuaIeAQYFgoAIBYhBIGNUH8eYhOfihfqpkYj3qBv2s/hBQJp
EFyhAhsgAAoJEEYj3qBv2s/hC/gBALzCqh3ctzvb7oIDlhZ/9n87Fk+fxKiNTHRC
LUBBsmBvAPsFIQSN4XV0wPlCJQ/FJuDnd5paKiHfvnP8PNOdoEUEB7g4BGkQXKES
CisGAQQBl1UBBQEBB0BKyo77IRJos8VOc0jpb2hFhU155xLyNLPzCfLCd1a+cQMB
CAeIeAQYFgoAIBYhBIGNUH8eYhOfihfqpkYj3qBv2s/hBQJpEFyhAhsMAAoJEEYj
3qBv2s/h4eYA+gOEnTCwshQcPJDVviq6+K/E3cEqpX9zP9q8PiiAONIMAP4kv8us
MxverdzrWh20hxbY1ESMO55STiJlCofv54bICg==
=vJ/Z
-----END PGP PUBLIC KEY BLOCK-----
"#;

thread_local! {
    // libtest runs every test on its own thread, so this directory and every
    // key file and data home created in it are removed when the test
    // finishes, including after a panic. Fixtures are created on the thread
    // that runs the command; do not hand them to another thread.
    static SCRATCH: TempDir = tempfile::Builder::new()
        .prefix("simit-test-")
        .tempdir()
        .expect("create per-test scratch directory");
    static DATA_HOMES: Cell<u32> = const { Cell::new(0) };
}

pub fn maintainer_key_path() -> PathBuf {
    SCRATCH.with(|scratch| {
        let path = scratch.path().join("maintainers.asc");
        if !path.exists() {
            fs::write(&path, MAINTAINERS_ASC).expect("write maintainer key fixture");
        }
        path
    })
}

/// A fresh, empty data home for each call, so commands never share state.
pub fn data_home_path() -> PathBuf {
    let index = DATA_HOMES.with(|count| {
        let index = count.get();
        count.set(index + 1);
        index
    });
    SCRATCH.with(|scratch| {
        let path = scratch.path().join(format!("data-home-{index}"));
        fs::create_dir(&path).expect("create isolated simit data home");
        path
    })
}

pub fn simit() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_simit"));
    command.env("SIMIT_MAINTAINERS_GPG", maintainer_key_path());
    command.env("XDG_DATA_HOME", data_home_path());
    command
}
