//! Shared helpers for the integration tests.
//!
//! Every test gets its own temporary "user" directory (where scripts are
//! written) and its own cache directory, so tests never see each other's
//! generated projects and never touch the real `~/.cache/rustrun`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub cache: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        let f = Fixture { root: tempfile::tempdir().unwrap(), cache: tempfile::tempdir().unwrap() };
        std::fs::create_dir_all(f.base()).unwrap();
        f
    }

    /// The working directory for scripts. It is a subdirectory of the temp
    /// dir so tests can also write `../something` without escaping it.
    pub fn base(&self) -> PathBuf {
        self.root.path().join("w")
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.base().join(rel)
    }

    /// Write `contents` to `rel` (creating parent directories) and return its path.
    pub fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.path(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// A `rustrun` command that runs from the fixture root.
    pub fn cmd(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustrun"));
        cmd.current_dir(self.base())
            .env("RUSTRUN_CACHE_DIR", self.cache.path())
            // Any test that resolves versions must opt in to a fake index.
            .env("RUSTRUN_INDEX", self.root.path().join("no-index-configured"))
            .env_remove("CARGO_TARGET_DIR");
        cmd
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).output().unwrap()
    }

    /// Run and assert success, returning stdout.
    pub fn run_ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert_success(&out);
        String::from_utf8(out.stdout).unwrap()
    }

    /// The hidden project directories rustrun has created so far.
    pub fn projects(&self) -> Vec<PathBuf> {
        let dir = self.cache.path().join("projects");
        let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
        let mut v: Vec<_> = rd.map(|e| e.unwrap().path()).collect();
        v.sort();
        v
    }
}

pub fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "rustrun failed with {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Lay out a fake sparse index: `crates` is `(name, [(version, rust_version, features)])`.
/// `(version, rust_version, features)`
pub type FakeVersion<'a> = (&'a str, Option<&'a str>, &'a [&'a str]);

pub fn write_index(dir: &Path, crates: &[(&str, &[FakeVersion])]) {
    for (name, versions) in crates {
        let rel = rustrun::resolve::index_path(name);
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lines: Vec<String> = versions
            .iter()
            .map(|(vers, rv, features)| {
                let features: serde_json::Map<String, serde_json::Value> =
                    features.iter().map(|f| (f.to_string(), serde_json::json!([]))).collect();
                serde_json::json!({
                    "name": name, "vers": vers, "deps": [], "cksum": "0",
                    "features": features, "yanked": false, "rust_version": rv,
                })
                .to_string()
            })
            .collect();
        std::fs::write(path, lines.join("\n")).unwrap();
    }
}

/// Snapshot of every file under `dir` with its contents, for "rustrun left
/// the user's directory untouched" assertions.
pub fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push((p.strip_prefix(dir).unwrap().to_path_buf(), std::fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}
