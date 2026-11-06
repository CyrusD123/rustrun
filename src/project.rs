//! Where each script's hidden Cargo project lives, and what it is called.

use std::path::{Path, PathBuf};

/// 64-bit FNV-1a. Used for cache directory names, so it must be stable
/// across Rust releases (unlike `std`'s `DefaultHasher`).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Names Cargo refuses (or that collide with directories inside `target/`).
const RESERVED: &[&str] = &[
    // Rust keywords
    "as",
    "async",
    "await",
    "break",
    "const",
    "continue",
    "crate",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    "abstract",
    "become",
    "box",
    "do",
    "final",
    "gen",
    "macro",
    "override",
    "priv",
    "try",
    "typeof",
    "unsized",
    "virtual",
    "yield",
    // Built-in crates
    "std",
    "core",
    "alloc",
    "proc_macro",
    "proc-macro",
    "test",
    // target/ layout and Windows device names
    "build",
    "deps",
    "examples",
    "incremental",
    "con",
    "prn",
    "aux",
    "nul",
    "com1",
    "com2",
    "com3",
    "com4",
    "com5",
    "com6",
    "com7",
    "com8",
    "com9",
    "lpt1",
    "lpt2",
    "lpt3",
    "lpt4",
    "lpt5",
    "lpt6",
    "lpt7",
    "lpt8",
    "lpt9",
];

/// Turn a file stem into a valid Cargo package/binary name.
///
/// `dep_names` are the script's dependencies: a binary that shares its crate
/// name with a dependency's library makes `use dep::...` ambiguous, so those
/// get a suffix too.
pub fn sanitize_name(stem: &str, dep_names: &[&str]) -> String {
    let mut name: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        name.insert_str(0, "script_");
    }
    let normalized = |s: &str| s.to_ascii_lowercase().replace('-', "_");
    let lower = normalized(&name);
    if RESERVED.iter().any(|r| normalized(r) == lower) || dep_names.iter().any(|d| normalized(d) == lower) {
        name.push_str("_script");
    }
    name
}

/// Root of all of rustrun's cached projects.
pub fn default_cache_root() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("rustrun")
}

/// The hidden project directory for one script, keyed on its canonical path
/// so that two `main.rs` files in different directories never collide.
pub fn project_dir(cache_root: &Path, canonical_script: &Path) -> PathBuf {
    let stem = canonical_script.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
    let hash = fnv1a64(canonical_script.as_os_str().as_encoded_bytes());
    cache_root.join("projects").join(format!("{}-{hash:016x}", sanitize_name(&stem, &[])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_known_values() {
        assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a64(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn sanitizing() {
        assert_eq!(sanitize_name("hello", &[]), "hello");
        assert_eq!(sanitize_name("my-script", &[]), "my-script");
        assert_eq!(sanitize_name("my script.v2", &[]), "my_script_v2");
        assert_eq!(sanitize_name("2fast", &[]), "script_2fast");
        assert_eq!(sanitize_name("_private", &[]), "script__private");
        assert_eq!(sanitize_name("", &[]), "script_");
        assert_eq!(sanitize_name("ünï", &[]), "script__n_");
        assert_eq!(sanitize_name("test", &[]), "test_script");
        assert_eq!(sanitize_name("fn", &[]), "fn_script");
        assert_eq!(sanitize_name("Deps", &[]), "Deps_script");
        assert_eq!(sanitize_name("serde", &["serde"]), "serde_script");
        assert_eq!(sanitize_name("proc_macro2", &["proc-macro2"]), "proc_macro2_script");
    }

    #[test]
    fn project_dirs_are_distinct_and_stable() {
        let root = Path::new("/cache");
        let a = project_dir(root, Path::new("/x/main.rs"));
        let b = project_dir(root, Path::new("/y/main.rs"));
        assert_ne!(a, b);
        assert_eq!(a, project_dir(root, Path::new("/x/main.rs")));
        assert!(a.starts_with("/cache/projects"));
        assert!(a.file_name().unwrap().to_str().unwrap().starts_with("main-"));
    }
}
