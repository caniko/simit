//! Recognition of the supported static hook contract. This is not evidence
//! that a hook is installed or that its supplied wrapper executes correctly.

use std::collections::BTreeMap;

use rnix::ast::{Attr, AttrSet, Entry, Expr, HasEntry, Param};

use super::unparen;

fn literal_attrs(set: AttrSet) -> Option<BTreeMap<String, Expr>> {
    if set.rec_token().is_some() {
        return None;
    }
    let mut result = BTreeMap::new();
    for entry in set.entries() {
        let Entry::AttrpathValue(value) = entry else {
            return None;
        };
        let path = value.attrpath()?;
        let mut attrs = path.attrs();
        let Attr::Ident(name) = attrs.next()? else {
            return None;
        };
        if attrs.next().is_some()
            || result
                .insert(name.to_string(), unparen(value.value()?)?)
                .is_some()
        {
            return None;
        }
    }
    Some(result)
}

fn tokens(source: &str) -> Vec<(rnix::SyntaxKind, String)> {
    rnix::Root::parse(source)
        .syntax()
        .descendants_with_tokens()
        .filter_map(|node| {
            let token = node.into_token()?;
            (!matches!(
                token.kind(),
                rnix::SyntaxKind::TOKEN_WHITESPACE | rnix::SyntaxKind::TOKEN_COMMENT
            ))
            .then(|| (token.kind(), token.text().to_owned()))
        })
        .collect()
}

fn recognize(source: &str) -> Option<()> {
    let parsed = rnix::Root::parse(source);
    if !parsed.errors().is_empty() {
        return None;
    }
    let Expr::Lambda(lambda) = unparen(parsed.tree().expr()?)? else {
        return None;
    };
    let Param::Pattern(pattern) = lambda.param()? else {
        return None;
    };
    // Bind the command's interpolation to the required caller-supplied wrapper.
    // Defaults, aliases and recursive sets can shadow that binding.
    if pattern.pat_bind().is_some()
        || pattern.pat_entries().any(|entry| {
            entry
                .ident()
                .is_some_and(|id| matches!(id.to_string().as_str(), "true" | "false"))
        })
    {
        return None;
    }
    let wrappers = pattern
        .pat_entries()
        .filter(|entry| {
            entry
                .ident()
                .is_some_and(|id| id.to_string() == "treefmtWrapper")
        })
        .collect::<Vec<_>>();
    if wrappers.len() != 1 || wrappers[0].default().is_some() {
        return None;
    }
    let Expr::AttrSet(body) = unparen(lambda.body()?)? else {
        return None;
    };
    let mut hooks = literal_attrs(body)?;
    if hooks.contains_key("cargo-fmt") || hooks.contains_key("uv-ruff-format") {
        return None;
    }
    let Expr::AttrSet(hook) = hooks.remove("treefmt")? else {
        return None;
    };
    let fields = literal_attrs(hook)?;
    for (name, expected) in [
        ("enable", "true"),
        ("entry", r#""${treefmtWrapper}/bin/treefmt --ci""#),
        ("pass_filenames", "false"),
    ] {
        if tokens(&fields.get(name)?.to_string()) != tokens(expected) {
            return None;
        }
    }
    // Additional filters, stages, arguments or dynamic overrides need evaluated
    // evidence. Never certify a hook that silently narrows the generated scope.
    for (name, value) in fields {
        match name.as_str() {
            "enable" | "entry" | "pass_filenames" => {}
            "package" if tokens(&value.to_string()) == tokens("treefmtWrapper") => {}
            "name" if matches!(value, Expr::Str(ref s) if !s.to_string().contains("${")) => {}
            _ => return None,
        }
    }
    Some(())
}

pub(super) fn matches(source: &str) -> bool {
    recognize(source).is_some()
}

#[cfg(test)]
mod tests {
    use super::matches;

    const VALID: &str = r#"{pkgs, treefmtWrapper, ...}: {
  # cargo-fmt and uv-ruff-format were replaced by this hook.
  treefmt = {
    name = "Project formatting";
    package = treefmtWrapper;
    pass_filenames = false;
    entry = "${ treefmtWrapper }/bin/treefmt --ci";
    enable = (true);
  };
}"#;

    #[test]
    fn accepts_static_contract_with_comments_reordering_and_optional_metadata() {
        assert!(matches(VALID));
    }

    #[test]
    fn rejects_overrides_shadowing_and_unsupported_indirection() {
        for (from, to) in [
            ("enable = (true);", "enable = true; enable = false;"),
            ("treefmt = {", "treefmt.enable = false; treefmt = {"),
            ("treefmt = {", "cargo-fmt = {}; treefmt = {"),
            ("treefmt = {", "uv-ruff-format = {}; treefmt = {"),
            ("treefmt = {", "treefmt = rec {"),
            ("treefmt = {", "treefmt = pkgs.lib.mkForce {"),
            ("treefmt = {", "inherit (pkgs) treefmt; ignored = {"),
            (
                "treefmtWrapper, ...",
                "treefmtWrapper ? pkgs.otherWrapper, ...",
            ),
            ("treefmtWrapper, ...", "treefmtWrapper, true ? false, ..."),
            ("treefmtWrapper, ...", "treefmtWrapper, false ? true, ..."),
            ("package = treefmtWrapper;", "package = pkgs.otherWrapper;"),
            (
                "pass_filenames = false;",
                "pass_filenames = false; args = [\"--allow-missing-formatter\"];",
            ),
        ] {
            assert!(VALID.contains(from));
            assert!(!matches(&VALID.replacen(from, to, 1)), "accepted {to}");
        }
    }
}
