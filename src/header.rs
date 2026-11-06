//! Parsing of the slimmed-down dependency header.
//!
//! The header is an ordinary Rust comment at the top of the script, so rustc
//! compiles the file completely unmodified (this is what lets us point Cargo
//! at the original file instead of a copy). Only the *leading* comment block
//! is inspected: the optional shebang line, blank lines, comments and inner
//! attributes (`#![...]`) that appear before the first line of real code.
//!
//! ```text
//! #!/usr/bin/env rustrun
//! // deps: rand, serde[derive], serde_json
//! // deps: regex@1.10
//! // edition: 2024
//! ```
//!
//! Grammar of a single dependency (entries are comma separated, and a script
//! may contain any number of `deps:` lines):
//!
//! ```text
//! name            -> latest compatible version is resolved from crates.io
//! name@REQ        -> REQ is any Cargo version requirement, used verbatim
//! name[f1, f2]    -> enable features f1 and f2
//! name@REQ[f1]    -> both (the `[..]` and `@..` parts may appear in either order)
//! ```

use anyhow::{Context, Result, bail};

/// One dependency as written in the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepSpec {
    /// Crate name as written by the user (may use `_` where crates.io uses `-`).
    pub name: String,
    /// Explicit version requirement, if the user gave one.
    pub req: Option<String>,
    /// Features to enable, in the order first written, deduplicated.
    pub features: Vec<String>,
}

impl DepSpec {
    pub fn new(name: &str) -> Self {
        DepSpec { name: name.to_string(), req: None, features: Vec::new() }
    }
}

/// Everything we understand from a script's leading comment block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    pub deps: Vec<DepSpec>,
    pub edition: Option<String>,
}

const EDITIONS: &[&str] = &["2015", "2018", "2021", "2024"];

/// Parse the header out of a full script source.
pub fn parse(source: &str) -> Result<Header> {
    let mut header = Header::default();
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut in_block = false;

    for (idx, raw) in source.lines().enumerate() {
        let lineno = idx + 1;
        let line = raw.trim();

        // rustc only treats `#!` as a shebang on the very first line, and only
        // when it is not the start of an inner attribute (`#![...]`).
        if idx == 0 && line.starts_with("#!") && !line.starts_with("#![") {
            continue;
        }

        let text: &str = if in_block {
            let body = match line.find("*/") {
                Some(end) => {
                    in_block = false;
                    &line[..end]
                }
                None => line,
            };
            // Allow the conventional ` * deps: ...` continuation style.
            body.trim_start().trim_start_matches('*')
        } else if line.is_empty() || line.starts_with("#![") {
            continue;
        } else if let Some(rest) = line.strip_prefix("//") {
            // `//`, `///` and `//!` are all accepted.
            rest.trim_start_matches(['/', '!'])
        } else if let Some(rest) = line.strip_prefix("/*") {
            let rest = rest.trim_start_matches(['*', '!']);
            match rest.find("*/") {
                Some(end) => &rest[..end],
                None => {
                    in_block = true;
                    rest
                }
            }
        } else {
            // First line of real code: the header is over.
            break;
        };

        apply_directive(&mut header, text.trim())
            .with_context(|| format!("invalid header on line {lineno}: `{line}`"))?;
    }

    Ok(header)
}

fn apply_directive(header: &mut Header, text: &str) -> Result<()> {
    if let Some(list) = strip_keyword(text, "deps") {
        for item in split_top_level(list)? {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            merge(&mut header.deps, parse_dep(item)?)?;
        }
    } else if let Some(value) = strip_keyword(text, "edition") {
        let value = value.trim();
        if !EDITIONS.contains(&value) {
            bail!("unknown edition `{value}` (expected one of {})", EDITIONS.join(", "));
        }
        header.edition = Some(value.to_string());
    }
    Ok(())
}

/// Matches `keyword:` case-insensitively at the start of `text`.
fn strip_keyword<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    let head = text.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    text[keyword.len()..].trim_start().strip_prefix(':')
}

/// Split on commas that are not inside `[...]`.
fn split_top_level(list: &str) -> Result<Vec<&str>> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in list.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth = depth.checked_sub(1).context("unmatched `]`")?;
            }
            ',' if depth == 0 => {
                out.push(&list[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        bail!("unclosed `[` in feature list");
    }
    out.push(&list[start..]);
    Ok(out)
}

/// Parse a single entry such as `serde@1.0[derive, rc]`.
pub fn parse_dep(item: &str) -> Result<DepSpec> {
    let item = item.trim();
    let name_end = item.find(['@', '[']).unwrap_or(item.len());
    let name = item[..name_end].trim();
    validate_crate_name(name)?;

    let mut spec = DepSpec::new(name);
    let mut rest = item[name_end..].trim_start();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('@') {
            if spec.req.is_some() {
                bail!("`{name}` has more than one version requirement");
            }
            let end = after.find(['[', '@']).unwrap_or(after.len());
            let req = after[..end].trim();
            if req.is_empty() {
                bail!("`{name}@` is missing a version requirement");
            }
            semver::VersionReq::parse(req)
                .with_context(|| format!("`{req}` is not a valid version requirement"))?;
            spec.req = Some(req.to_string());
            rest = after[end..].trim_start();
        } else if let Some(after) = rest.strip_prefix('[') {
            let end = after.find(']').context("unclosed `[` in feature list")?;
            for feature in after[..end].split(',').map(str::trim).filter(|f| !f.is_empty()) {
                validate_feature_name(feature)?;
                if !spec.features.iter().any(|f| f == feature) {
                    spec.features.push(feature.to_string());
                }
            }
            rest = after[end + 1..].trim_start();
        } else {
            bail!("unexpected `{rest}` after `{name}`");
        }
    }
    Ok(spec)
}

/// Combine a repeated mention of the same crate with an earlier one.
fn merge(deps: &mut Vec<DepSpec>, new: DepSpec) -> Result<()> {
    let Some(existing) = deps.iter_mut().find(|d| d.name == new.name) else {
        deps.push(new);
        return Ok(());
    };
    match (&existing.req, new.req) {
        (Some(a), Some(b)) if *a != b => {
            bail!("`{}` is listed with conflicting versions `{a}` and `{b}`", new.name)
        }
        (None, Some(b)) => existing.req = Some(b),
        _ => {}
    }
    for f in new.features {
        if !existing.features.contains(&f) {
            existing.features.push(f);
        }
    }
    Ok(())
}

/// Same rules crates.io applies to new crate names.
pub fn validate_crate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("missing crate name");
    }
    if name.contains(char::is_whitespace) {
        bail!("invalid crate name `{name}` (separate dependencies with commas)");
    }
    if name.len() > 64 {
        bail!("crate name `{name}` is longer than 64 characters");
    }
    let mut chars = name.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        bail!("invalid crate name `{name}`: must start with an ASCII letter");
    }
    if let Some(bad) = chars.find(|c| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-')) {
        bail!("invalid character `{bad}` in crate name `{name}`");
    }
    Ok(())
}

fn validate_feature_name(feature: &str) -> Result<()> {
    let ok = feature.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '+' | '.' | '/' | ':'));
    if !ok {
        bail!("invalid feature name `{feature}`");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(name: &str, req: Option<&str>, features: &[&str]) -> DepSpec {
        DepSpec {
            name: name.into(),
            req: req.map(Into::into),
            features: features.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn deps(src: &str) -> Vec<DepSpec> {
        parse(src).unwrap().deps
    }

    #[test]
    fn no_header_means_no_deps() {
        assert_eq!(parse("fn main() {}\n").unwrap(), Header::default());
        assert_eq!(parse("").unwrap(), Header::default());
    }

    #[test]
    fn simple_list() {
        assert_eq!(
            deps("// deps: rand, serde_json\nfn main() {}"),
            vec![dep("rand", None, &[]), dep("serde_json", None, &[])]
        );
    }

    #[test]
    fn versions_and_features() {
        assert_eq!(
            deps("// deps: regex@1.10, serde[derive, rc], tokio@1[macros,rt-multi-thread]"),
            vec![
                dep("regex", Some("1.10"), &[]),
                dep("serde", None, &["derive", "rc"]),
                dep("tokio", Some("1"), &["macros", "rt-multi-thread"]),
            ]
        );
    }

    #[test]
    fn features_before_version() {
        assert_eq!(deps("// deps: serde[derive]@1.0"), vec![dep("serde", Some("1.0"), &["derive"])]);
    }

    #[test]
    fn single_comparator_requirements() {
        assert_eq!(
            deps("// deps: a@>=1.2, b@=0.3.1, c@~0.4, d@*"),
            vec![
                dep("a", Some(">=1.2"), &[]),
                dep("b", Some("=0.3.1"), &[]),
                dep("c", Some("~0.4"), &[]),
                dep("d", Some("*"), &[]),
            ]
        );
    }

    #[test]
    fn multi_comparator_requirements_are_rejected() {
        // A comma inside a requirement is ambiguous with the list separator,
        // so `<1.5` is parsed as a crate name and rejected. Documented
        // limitation of the first-approximation header format.
        let err = format!("{:#}", parse("// deps: a@>=1.2, <1.5").unwrap_err());
        assert!(err.contains("must start with an ASCII letter"), "{err}");
    }

    #[test]
    fn multiple_lines_accumulate() {
        let src = "// deps: rand\n// deps: serde\n\nfn main() {}";
        assert_eq!(deps(src), vec![dep("rand", None, &[]), dep("serde", None, &[])]);
    }

    #[test]
    fn duplicate_mentions_merge() {
        let src = "// deps: serde[derive]\n// deps: serde@1[rc], serde[derive]";
        assert_eq!(deps(src), vec![dep("serde", Some("1"), &["derive", "rc"])]);
    }

    #[test]
    fn conflicting_versions_are_an_error() {
        let err = parse("// deps: rand@0.8, rand@0.9").unwrap_err();
        assert!(format!("{err:#}").contains("conflicting versions"), "{err:#}");
    }

    #[test]
    fn shebang_is_skipped() {
        let src = "#!/usr/bin/env rustrun\n// deps: rand\nfn main() {}";
        assert_eq!(deps(src), vec![dep("rand", None, &[])]);
    }

    #[test]
    fn inner_attribute_on_first_line_is_not_a_shebang() {
        let src = "#![allow(unused)]\n// deps: rand\nfn main() {}";
        assert_eq!(deps(src), vec![dep("rand", None, &[])]);
    }

    #[test]
    fn doc_comment_styles() {
        assert_eq!(
            deps("//! deps: a\n/// deps: b\n//deps:c"),
            vec![dep("a", None, &[]), dep("b", None, &[]), dep("c", None, &[]),]
        );
    }

    #[test]
    fn keyword_is_case_insensitive() {
        assert_eq!(deps("// Deps: a\n// DEPS : b"), vec![dep("a", None, &[]), dep("b", None, &[])]);
    }

    #[test]
    fn block_comments() {
        let src = "/*\n * deps: rand,\n *   serde[derive]\n */\nfn main() {}";
        // A dependency list does not continue onto the next line; each line
        // needs its own `deps:` keyword. The second line is just prose.
        assert_eq!(deps(src), vec![dep("rand", None, &[])]);

        let src = "/*!\n deps: rand\n deps: serde[derive]\n*/\nfn main() {}";
        assert_eq!(deps(src), vec![dep("rand", None, &[]), dep("serde", None, &["derive"])]);

        assert_eq!(deps("/* deps: rand */ \nfn main() {}"), vec![dep("rand", None, &[])]);
    }

    #[test]
    fn only_leading_comments_count() {
        let src = "// deps: rand\nuse rand::Rng;\n// deps: serde\nfn main() {}";
        assert_eq!(deps(src), vec![dep("rand", None, &[])]);
    }

    #[test]
    fn prose_comments_are_ignored() {
        let src = "// A little script.\n// It depends on: nothing special\n// deps: rand\n";
        assert_eq!(deps(src), vec![dep("rand", None, &[])]);
    }

    #[test]
    fn trailing_commas_and_whitespace() {
        assert_eq!(
            deps("//   deps:   rand ,  , serde ,  "),
            vec![dep("rand", None, &[]), dep("serde", None, &[])]
        );
    }

    #[test]
    fn edition() {
        assert_eq!(parse("// edition: 2024").unwrap().edition.as_deref(), Some("2024"));
        assert!(parse("// edition: 2027").is_err());
    }

    #[test]
    fn crlf_and_bom() {
        let src = "\u{feff}#!/usr/bin/env rustrun\r\n// deps: rand\r\nfn main() {}\r\n";
        assert_eq!(deps(src), vec![dep("rand", None, &[])]);
    }

    #[test]
    fn bad_entries_are_errors_with_line_numbers() {
        for (src, needle) in [
            ("\n// deps: serde rand", "separate dependencies with commas"),
            ("// deps: 3d", "must start with an ASCII letter"),
            ("// deps: foo.bar", "invalid character"),
            ("// deps: serde[derive", "unclosed `[`"),
            ("// deps: serde]", "unmatched `]`"),
            ("// deps: serde@", "missing a version requirement"),
            ("// deps: serde@not-a-version", "not a valid version requirement"),
            ("// deps: serde[de rive]", "invalid feature name"),
            ("// deps: serde@1@2", "more than one version requirement"),
            ("// deps: serde[derive]x", "unexpected `x`"),
        ] {
            let err = format!("{:#}", parse(src).unwrap_err());
            assert!(err.contains(needle), "for {src:?} expected {needle:?} in {err:?}");
            assert!(err.contains("line"), "{err}");
        }
        let err = format!("{:#}", parse("\n// deps: serde rand").unwrap_err());
        assert!(err.contains("line 2"), "{err}");
    }
}
