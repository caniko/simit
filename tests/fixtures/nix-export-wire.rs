//! Independent fixture encoder following the documented Nix export/NAR grammar.
pub(crate) fn path(hash: char, name: &str) -> String {
    format!("/nix/store/{}-{name}", hash.to_string().repeat(32))
}
pub(crate) fn number(n: u64) -> Vec<u8> {
    n.to_le_bytes().to_vec()
}
pub(crate) fn string(bytes: impl AsRef<[u8]>) -> Vec<u8> {
    let bytes = bytes.as_ref();
    let mut raw = number(bytes.len() as u64);
    raw.extend(bytes);
    raw.resize(raw.len() + (8 - bytes.len() % 8) % 8, 0);
    raw
}
pub(crate) fn regular(contents: &[u8], executable: bool) -> Vec<u8> {
    let mut node = [string("("), string("type"), string("regular")].concat();
    if executable {
        node.extend([string("executable"), string("")].concat());
    }
    node.extend([string("contents"), string(contents), string(")")].concat());
    node
}
pub(crate) fn link(target: &[u8]) -> Vec<u8> {
    [
        string("("),
        string("type"),
        string("symlink"),
        string("target"),
        string(target),
        string(")"),
    ]
    .concat()
}
pub(crate) fn directory(entries: Vec<(Vec<u8>, Vec<u8>)>) -> Vec<u8> {
    let mut raw = [string("("), string("type"), string("directory")].concat();
    for (name, child) in entries {
        raw.extend(
            [
                string("entry"),
                string("("),
                string("name"),
                string(name),
                string("node"),
                child,
                string(")"),
            ]
            .concat(),
        );
    }
    raw.extend(string(")"));
    raw
}
pub(crate) fn depth(count: usize) -> Vec<u8> {
    let mut raw = regular(b"input", false);
    for _ in 0..count {
        raw = directory(vec![(b"a".to_vec(), raw)]);
    }
    raw
}
pub(crate) fn record(path: &str, node: Vec<u8>, references: &[String], deriver: &str) -> Vec<u8> {
    let mut raw = [
        number(1),
        string("nix-archive-1"),
        node,
        number(0x4558_494e),
        string(path),
        number(references.len() as u64),
    ]
    .concat();
    for reference in references {
        raw.extend(string(reference));
    }
    raw.extend([string(deriver), number(0)].concat());
    raw
}
pub(crate) fn export(records: Vec<Vec<u8>>) -> Vec<u8> {
    let mut raw: Vec<_> = records.into_iter().flatten().collect();
    raw.extend(number(0));
    raw
}
