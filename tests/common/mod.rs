use std::fs;
use std::path::PathBuf;
use std::process::{Command, id};
use std::time::{SystemTime, UNIX_EPOCH};

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

pub fn maintainer_key_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before UNIX_EPOCH")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("simit-maintainers-{}-{nanos}.asc", id()));
    fs::write(&path, MAINTAINERS_ASC).expect("write maintainer key fixture");
    path
}

pub fn data_home_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before UNIX_EPOCH")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("simit-data-home-{}-{nanos}", id()));
    fs::create_dir_all(&path).expect("create isolated simit data home");
    path
}

pub fn simit() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_simit"));
    command.env("SIMIT_MAINTAINERS_GPG", maintainer_key_path());
    command.env("XDG_DATA_HOME", data_home_path());
    command
}
