//! Auto-deriving dependency versions.
//!
//! For every header dependency without an explicit `@REQ`, we pick a version
//! the same way `cargo add` does: read the crate's entry in the crates.io
//! *sparse index* and choose the newest version that is
//!
//! 1. not yanked,
//! 2. stable (pre-releases are only used if nothing else exists),
//! 3. compatible with the active toolchain's `rustc` (its `rust-version`),
//!    falling back to the newest version if none is, with a warning.
//!
//! The chosen version is written to the generated manifest as a caret
//! requirement (`serde = "1.0.229"`) and remembered in the script's cache
//! directory, so later runs are offline and reproducible until `--update`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::header::DepSpec;

pub const DEFAULT_INDEX: &str = "https://index.crates.io";

/// One line of a sparse-index file (only the fields we need).
#[derive(Debug, Clone, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    pub vers: String,
    #[serde(default)]
    pub deps: Vec<IndexDep>,
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub features2: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default)]
    pub yanked: bool,
    #[serde(default)]
    pub rust_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexDep {
    pub name: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub kind: Option<String>,
}

impl IndexEntry {
    /// All feature names a dependent may enable on this version, including
    /// the implicit features created by optional dependencies.
    pub fn available_features(&self) -> BTreeSet<String> {
        let explicit = self.features.iter().chain(self.features2.iter().flatten());
        let mut out: BTreeSet<String> = explicit.clone().map(|(k, _)| k.clone()).collect();
        let uses_dep_syntax: BTreeSet<&str> =
            explicit.flat_map(|(_, v)| v.iter()).filter_map(|v| v.strip_prefix("dep:")).collect();
        for dep in &self.deps {
            let dev = dep.kind.as_deref() == Some("dev");
            if dep.optional && !dev && !uses_dep_syntax.contains(dep.name.as_str()) {
                out.insert(dep.name.clone());
            }
        }
        out
    }
}

/// Parse a sparse-index file (newline-delimited JSON). Lines in a schema we
/// cannot read are skipped, as Cargo itself does.
pub fn parse_index_file(text: &str) -> Vec<IndexEntry> {
    text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Relative location of a crate inside a sparse (or git) registry index.
pub fn index_path(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    }
}

/// Somewhere we can look up a crate's published versions.
pub trait VersionSource {
    /// `Ok(None)` means the crate does not exist.
    fn fetch(&self, name: &str) -> Result<Option<Vec<IndexEntry>>>;
}

/// A sparse registry index reached over HTTP(S), or laid out on local disk
/// (used by the tests, and handy for mirrors).
pub struct SparseIndex {
    base: String,
    agent: Option<ureq::Agent>,
}

impl SparseIndex {
    pub fn new(base: &str) -> Self {
        let base = base.trim_end_matches('/').to_string();
        let agent = (base.starts_with("http://") || base.starts_with("https://")).then(|| {
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(30)))
                .user_agent(concat!("rustrun/", env!("CARGO_PKG_VERSION")))
                .build()
                .into()
        });
        SparseIndex { base, agent }
    }
}

impl VersionSource for SparseIndex {
    fn fetch(&self, name: &str) -> Result<Option<Vec<IndexEntry>>> {
        let rel = index_path(name);
        let Some(agent) = &self.agent else {
            let path = PathBuf::from(&self.base).join(rel);
            return match std::fs::read_to_string(&path) {
                Ok(text) => Ok(Some(parse_index_file(&text))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
            };
        };
        let url = format!("{}/{rel}", self.base);
        match agent.get(&url).call() {
            Ok(mut resp) => {
                let text = resp
                    .body_mut()
                    .with_config()
                    .limit(64 * 1024 * 1024)
                    .read_to_string()
                    .with_context(|| format!("reading response from {url}"))?;
                Ok(Some(parse_index_file(&text)))
            }
            // Same set of statuses Cargo treats as "no such crate".
            Err(ureq::Error::StatusCode(404 | 403 | 410 | 451)) => Ok(None),
            Err(e) => Err(e).with_context(|| format!("failed to query {url}")),
        }
    }
}

/// Look a crate up, tolerating `_` vs `-` confusion (`use proc_macro2` is
/// the `proc-macro2` crate). Returns the canonical name and its versions.
pub fn lookup(source: &dyn VersionSource, name: &str) -> Result<(String, Vec<IndexEntry>)> {
    let mut candidates = vec![name.to_string()];
    for alt in [name.replace('_', "-"), name.replace('-', "_")] {
        if !candidates.contains(&alt) {
            candidates.push(alt);
        }
    }
    for candidate in &candidates {
        if let Some(entries) = source.fetch(candidate)?
            && let Some(first) = entries.first()
        {
            return Ok((first.name.clone(), entries));
        }
    }
    bail!("could not find crate `{name}` on crates.io")
}

/// Parse `rust-version` values, which may omit minor/patch (`"1.70"`).
pub fn parse_partial_version(s: &str) -> Option<Version> {
    let core = s.trim().split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>());
    let major = parts.next()?.ok()?;
    let minor = parts.next().transpose().ok()?.unwrap_or(0);
    let patch = parts.next().transpose().ok()?.unwrap_or(0);
    if parts.next().is_some() {
        return None;
    }
    Some(Version::new(major, minor, patch))
}

#[derive(Debug, Clone)]
pub struct Selection<'a> {
    pub version: Version,
    pub entry: &'a IndexEntry,
    /// No version was compatible with the current rustc, so the newest was used.
    pub ignored_msrv: bool,
}

/// Choose the version `cargo add` would.
pub fn select_version<'a>(
    entries: &'a [IndexEntry],
    req: Option<&VersionReq>,
    rustc: Option<&Version>,
) -> Result<Selection<'a>> {
    let mut candidates: Vec<(Version, &IndexEntry)> = entries
        .iter()
        .filter(|e| !e.yanked)
        .filter_map(|e| Version::parse(&e.vers).ok().map(|v| (v, e)))
        .filter(|(v, _)| req.is_none_or(|r| r.matches(v)))
        .collect();
    if candidates.is_empty() {
        let name = entries.first().map_or("<unknown>", |e| &e.name);
        match req {
            Some(r) => bail!("no published, non-yanked version of `{name}` matches `{r}`"),
            None => bail!("`{name}` has no published, non-yanked versions"),
        }
    }
    if candidates.iter().any(|(v, _)| v.pre.is_empty()) {
        candidates.retain(|(v, _)| v.pre.is_empty());
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));

    let compatible = |e: &IndexEntry| match (rustc, e.rust_version.as_deref()) {
        (Some(rustc), Some(msrv)) => parse_partial_version(msrv).is_none_or(|m| m <= *rustc),
        _ => true,
    };
    let (version, entry, ignored_msrv) = match candidates.iter().find(|(_, e)| compatible(e)) {
        Some((v, e)) => (v.clone(), *e, false),
        None => (candidates[0].0.clone(), candidates[0].1, true),
    };
    Ok(Selection { version, entry, ignored_msrv })
}

/// Fail early (with a useful list) instead of letting Cargo reject the
/// manifest with a less specific message.
pub fn check_features(entry: &IndexEntry, wanted: &[String]) -> Result<()> {
    let available = entry.available_features();
    let missing: Vec<&String> = wanted.iter().filter(|f| !available.contains(*f)).collect();
    if missing.is_empty() {
        return Ok(());
    }
    let list = |v: Vec<&String>| v.iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", ");
    let avail = if available.is_empty() {
        "it has no optional features".to_string()
    } else {
        format!("available: {}", list(available.iter().collect()))
    };
    bail!("{} {} does not have feature {} ({avail})", entry.name, entry.vers, list(missing))
}

/// A dependency ready to be written into `[dependencies]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDep {
    /// Canonical crates.io name.
    pub name: String,
    /// Version requirement to write.
    pub req: String,
    pub features: Vec<String>,
}

/// Remembered resolutions, stored as JSON next to the generated manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionCache {
    pub entries: BTreeMap<String, CachedResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedResolution {
    pub package: String,
    pub version: String,
    pub features: Vec<String>,
}

impl ResolutionCache {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json + "\n").with_context(|| format!("writing {}", path.display()))
    }
}

/// Resolve every header dependency. `source` is only called for crates that
/// have no explicit requirement and no (valid) cached resolution, so scripts
/// whose versions are already known never touch the network.
pub fn resolve(
    specs: &[DepSpec],
    source: &mut dyn FnMut() -> Result<Box<dyn VersionSource>>,
    rustc: Option<&Version>,
    cache: &mut ResolutionCache,
    update: bool,
    warnings: &mut Vec<String>,
) -> Result<Vec<ResolvedDep>> {
    let mut source_handle: Option<Box<dyn VersionSource>> = None;
    let mut out = Vec::with_capacity(specs.len());

    // Drop cache entries for crates no longer in the header.
    cache.entries.retain(|name, _| specs.iter().any(|s| &s.name == name && s.req.is_none()));

    for spec in specs {
        if let Some(req) = &spec.req {
            out.push(ResolvedDep {
                name: spec.name.clone(),
                req: req.clone(),
                features: spec.features.clone(),
            });
            continue;
        }

        if !update
            && let Some(hit) = cache.entries.get(&spec.name)
            && hit.features == spec.features
        {
            out.push(ResolvedDep {
                name: hit.package.clone(),
                req: hit.version.clone(),
                features: spec.features.clone(),
            });
            continue;
        }

        if source_handle.is_none() {
            source_handle = Some(source()?);
        }
        let src = source_handle.as_deref().unwrap();
        let (package, entries) = lookup(src, &spec.name)?;
        let sel = select_version(&entries, None, rustc)?;
        check_features(sel.entry, &spec.features)?;
        if sel.ignored_msrv {
            warnings.push(format!(
                "no version of `{package}` supports rustc {}; using {} (requires rustc {})",
                rustc.map_or_else(|| "?".to_string(), |v| v.to_string()),
                sel.version,
                sel.entry.rust_version.as_deref().unwrap_or("?"),
            ));
        }
        if package != spec.name {
            warnings.push(format!("using crate `{package}` for `{}`", spec.name));
        }
        let version = sel.version.to_string();
        cache.entries.insert(
            spec.name.clone(),
            CachedResolution {
                package: package.clone(),
                version: version.clone(),
                features: spec.features.clone(),
            },
        );
        out.push(ResolvedDep { name: package, req: version, features: spec.features.clone() });
    }
    Ok(out)
}

/// Version of the `rustc` Cargo will use (honours `$RUSTC`).
pub fn rustc_version() -> Option<Version> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let out = std::process::Command::new(rustc).arg("--version").output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    // "rustc 1.99.1 (58cf8f9c3 2026-10-13)" or "rustc 1.100.0-nightly (...)"
    parse_partial_version(text.split_whitespace().nth(1)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn entry(vers: &str) -> IndexEntry {
        IndexEntry {
            name: "demo".into(),
            vers: vers.into(),
            deps: vec![],
            features: BTreeMap::new(),
            features2: None,
            yanked: false,
            rust_version: None,
        }
    }

    fn yanked(vers: &str) -> IndexEntry {
        IndexEntry { yanked: true, ..entry(vers) }
    }

    fn msrv(vers: &str, rv: &str) -> IndexEntry {
        IndexEntry { rust_version: Some(rv.into()), ..entry(vers) }
    }

    fn pick(entries: &[IndexEntry], rustc: Option<&str>) -> String {
        let rustc = rustc.map(|r| Version::parse(r).unwrap());
        select_version(entries, None, rustc.as_ref()).unwrap().version.to_string()
    }

    #[test]
    fn index_paths_follow_cargo_layout() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("serde"), "se/rd/serde");
        assert_eq!(index_path("Inflector"), "in/fl/inflector");
    }

    #[test]
    fn parses_real_index_lines_and_skips_garbage() {
        let text = r#"{"name":"itoa","vers":"1.0.17","deps":[{"name":"no-panic","req":"^0.1","features":[],"optional":true,"default_features":true,"target":null,"kind":"normal"}],"cksum":"x","features":{},"yanked":false,"rust_version":"1.68"}
this is not json

{"name":"itoa","vers":"1.0.18","deps":[],"cksum":"y","features":{},"yanked":false,"v":2,"features2":{"x":[]}}"#;
        let entries = parse_index_file(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].rust_version.as_deref(), Some("1.68"));
        assert!(entries[1].features2.as_ref().unwrap().contains_key("x"));
    }

    #[test]
    fn picks_newest_stable() {
        let e = [entry("1.0.0"), entry("1.2.0"), entry("1.10.0"), entry("2.0.0-beta.1")];
        assert_eq!(pick(&e, None), "1.10.0");
    }

    #[test]
    fn skips_yanked() {
        assert_eq!(pick(&[entry("1.0.0"), yanked("1.1.0")], None), "1.0.0");
    }

    #[test]
    fn uses_prerelease_only_when_nothing_else() {
        assert_eq!(pick(&[entry("0.1.0-alpha.1"), entry("0.1.0-alpha.2")], None), "0.1.0-alpha.2");
    }

    #[test]
    fn respects_rust_version() {
        let e = [msrv("1.0.0", "1.60"), msrv("1.1.0", "1.80"), msrv("1.2.0", "1.95.1")];
        assert_eq!(pick(&e, Some("1.95.0")), "1.1.0");
        assert_eq!(pick(&e, Some("1.95.1")), "1.2.0");
        assert_eq!(pick(&e, None), "1.2.0");
    }

    #[test]
    fn falls_back_to_newest_when_nothing_supports_rustc() {
        let e = [msrv("1.0.0", "1.90"), msrv("1.1.0", "1.91")];
        let rustc = Version::new(1, 50, 0);
        let sel = select_version(&e, None, Some(&rustc)).unwrap();
        assert_eq!(sel.version.to_string(), "1.1.0");
        assert!(sel.ignored_msrv);
    }

    #[test]
    fn honours_requirement() {
        let e = [entry("0.7.3"), entry("0.8.5"), entry("0.9.0")];
        let req = VersionReq::parse("0.8").unwrap();
        assert_eq!(select_version(&e, Some(&req), None).unwrap().version.to_string(), "0.8.5");
        let req = VersionReq::parse("5").unwrap();
        assert!(select_version(&e, Some(&req), None).is_err());
    }

    #[test]
    fn all_yanked_is_an_error() {
        assert!(select_version(&[yanked("1.0.0")], None, None).is_err());
    }

    #[test]
    fn partial_versions() {
        assert_eq!(parse_partial_version("1.70"), Some(Version::new(1, 70, 0)));
        assert_eq!(parse_partial_version("1"), Some(Version::new(1, 0, 0)));
        assert_eq!(parse_partial_version("1.100.0-nightly"), Some(Version::new(1, 100, 0)));
        assert_eq!(parse_partial_version("1.2.3.4"), None);
        assert_eq!(parse_partial_version("one"), None);
    }

    #[test]
    fn features_include_implicit_optional_deps() {
        let mut e = entry("1.0.0");
        e.features.insert("derive".into(), vec!["serde_derive".into()]);
        e.features.insert("json".into(), vec!["dep:serde_json".into()]);
        e.deps = vec![
            IndexDep { name: "serde_derive".into(), optional: true, kind: Some("normal".into()) },
            // Referenced with `dep:`, so it does not get an implicit feature.
            IndexDep { name: "serde_json".into(), optional: true, kind: None },
            IndexDep { name: "required".into(), optional: false, kind: None },
        ];
        let avail = e.available_features();
        assert!(avail.contains("derive") && avail.contains("json") && avail.contains("serde_derive"));
        assert!(!avail.contains("serde_json") && !avail.contains("required"));

        assert!(check_features(&e, &["derive".into(), "serde_derive".into()]).is_ok());
        let err = check_features(&e, &["drive".into()]).unwrap_err().to_string();
        assert!(err.contains("`drive`") && err.contains("available: `derive`"), "{err}");
    }

    /// In-memory registry that counts how often it is consulted.
    struct FakeIndex {
        crates: BTreeMap<String, Vec<IndexEntry>>,
        calls: std::rc::Rc<Cell<usize>>,
    }

    impl VersionSource for FakeIndex {
        fn fetch(&self, name: &str) -> Result<Option<Vec<IndexEntry>>> {
            self.calls.set(self.calls.get() + 1);
            Ok(self.crates.get(name).cloned())
        }
    }

    fn fake(calls: &std::rc::Rc<Cell<usize>>) -> impl FnMut() -> Result<Box<dyn VersionSource>> {
        let calls = calls.clone();
        move || {
            let mut crates = BTreeMap::new();
            let mut e = entry("1.4.2");
            e.name = "proc-macro2".into();
            crates.insert("proc-macro2".to_string(), vec![e]);
            let mut s = entry("1.0.200");
            s.name = "serde".into();
            s.features.insert("derive".into(), vec![]);
            crates.insert("serde".to_string(), vec![s]);
            Ok(Box::new(FakeIndex { crates, calls: calls.clone() }) as Box<dyn VersionSource>)
        }
    }

    fn spec(s: &str) -> DepSpec {
        crate::header::parse_dep(s).unwrap()
    }

    #[test]
    fn resolve_end_to_end_with_cache() {
        let calls = std::rc::Rc::new(Cell::new(0));
        let mut cache = ResolutionCache::default();
        let mut warnings = vec![];
        let specs = [spec("serde[derive]"), spec("proc_macro2"), spec("rand@0.8")];

        let out = resolve(&specs, &mut fake(&calls), None, &mut cache, false, &mut warnings).unwrap();
        assert_eq!(
            out,
            vec![
                ResolvedDep { name: "serde".into(), req: "1.0.200".into(), features: vec!["derive".into()] },
                ResolvedDep { name: "proc-macro2".into(), req: "1.4.2".into(), features: vec![] },
                ResolvedDep { name: "rand".into(), req: "0.8".into(), features: vec![] },
            ]
        );
        assert!(warnings.iter().any(|w| w.contains("`proc-macro2` for `proc_macro2`")));
        assert!(!cache.entries.contains_key("rand"), "explicit versions are not cached");

        // Second run: fully served from the cache, the index is never built.
        let before = calls.get();
        let mut never = || -> Result<Box<dyn VersionSource>> { panic!("index should not be used") };
        let again = resolve(&specs, &mut never, None, &mut cache, false, &mut vec![]).unwrap();
        assert_eq!(again, out);
        assert_eq!(calls.get(), before);

        // `--update` goes back to the index.
        resolve(&specs, &mut fake(&calls), None, &mut cache, true, &mut vec![]).unwrap();
        assert!(calls.get() > before);
    }

    #[test]
    fn changing_features_invalidates_cache_entry() {
        let calls = std::rc::Rc::new(Cell::new(0));
        let mut cache = ResolutionCache::default();
        resolve(&[spec("serde")], &mut fake(&calls), None, &mut cache, false, &mut vec![]).unwrap();
        let n = calls.get();
        resolve(&[spec("serde[derive]")], &mut fake(&calls), None, &mut cache, false, &mut vec![]).unwrap();
        assert_eq!(calls.get(), n + 1);
    }

    #[test]
    fn removed_crates_leave_the_cache() {
        let calls = std::rc::Rc::new(Cell::new(0));
        let mut cache = ResolutionCache::default();
        resolve(
            &[spec("serde"), spec("proc-macro2")],
            &mut fake(&calls),
            None,
            &mut cache,
            false,
            &mut vec![],
        )
        .unwrap();
        resolve(&[spec("serde")], &mut fake(&calls), None, &mut cache, false, &mut vec![]).unwrap();
        assert_eq!(cache.entries.keys().collect::<Vec<_>>(), ["serde"]);
    }

    #[test]
    fn unknown_crate_and_bad_feature_errors() {
        let calls = std::rc::Rc::new(Cell::new(0));
        let mut cache = ResolutionCache::default();
        let err = resolve(&[spec("nope")], &mut fake(&calls), None, &mut cache, false, &mut vec![])
            .unwrap_err()
            .to_string();
        assert!(err.contains("could not find crate `nope`"), "{err}");
        let err = resolve(&[spec("serde[derp]")], &mut fake(&calls), None, &mut cache, false, &mut vec![])
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not have feature `derp`"), "{err}");
    }

    #[test]
    fn cache_round_trips_and_tolerates_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resolved.json");
        let mut cache = ResolutionCache::default();
        cache.entries.insert(
            "a".into(),
            CachedResolution { package: "a".into(), version: "1.0.0".into(), features: vec![] },
        );
        cache.save(&path).unwrap();
        assert_eq!(ResolutionCache::load(&path), cache);
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(ResolutionCache::load(&path), ResolutionCache::default());
        assert_eq!(ResolutionCache::load(&dir.path().join("missing")), ResolutionCache::default());
    }
}
