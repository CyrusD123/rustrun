//! rustrun: run a standalone Rust script without writing a Cargo.toml.
//!
//! First approximation: dependency *names* (and optionally features) are
//! listed in an ordinary Rust comment at the top of the script; versions are
//! derived automatically from crates.io. The generated manifest lives in a
//! cache directory and points Cargo at the original, unmodified script, so
//! compile-time relative paths (`mod`, `include!`, `include_str!`, ...) work.
//!
//! Scripts must define `fn main()`. Wrapping code in a synthetic `main` would
//! require compiling a modified copy, which breaks relative imports.

pub mod header;
pub mod manifest;
pub mod project;
pub mod resolve;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Everything the CLI can ask for.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub script: PathBuf,
    pub args: Vec<OsString>,
    pub release: bool,
    /// Print the generated manifest instead of building and running.
    pub print_manifest: bool,
    /// Re-resolve versions of crates without an explicit requirement.
    pub update: bool,
    /// Delete this script's cached project.
    pub clean: bool,
    pub cache_dir: Option<PathBuf>,
    /// Sparse index base URL, or a directory laid out like one.
    pub index: Option<String>,
}

/// Prepared state for one script: everything up to (not including) the build.
pub struct Prepared {
    pub script: PathBuf,
    pub project_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: String,
    pub bin_name: String,
    pub warnings: Vec<String>,
}

/// Parse the header, resolve versions and write the hidden project.
pub fn prepare(opts: &Options) -> Result<Prepared> {
    let script = opts
        .script
        .canonicalize()
        .with_context(|| format!("cannot find script `{}`", opts.script.display()))?;
    if !script.is_file() {
        bail!("`{}` is not a file", opts.script.display());
    }
    let source = std::fs::read_to_string(&script).with_context(|| format!("reading {}", script.display()))?;
    let header = header::parse(&source)?;

    let cache_root = opts.cache_dir.clone().unwrap_or_else(project::default_cache_root);
    let project_dir = project::project_dir(&cache_root, &script);
    std::fs::create_dir_all(&project_dir).with_context(|| format!("creating {}", project_dir.display()))?;

    let cache_path = project_dir.join("resolved.json");
    let mut cache = resolve::ResolutionCache::load(&cache_path);
    let index = opts.index.clone().unwrap_or_else(|| resolve::DEFAULT_INDEX.to_string());
    let mut source_factory =
        || -> Result<Box<dyn resolve::VersionSource>> { Ok(Box::new(resolve::SparseIndex::new(&index))) };
    let mut warnings = Vec::new();
    let rustc = if header.deps.iter().any(|d| d.req.is_none()) { resolve::rustc_version() } else { None };
    let deps = resolve::resolve(
        &header.deps,
        &mut source_factory,
        rustc.as_ref(),
        &mut cache,
        opts.update,
        &mut warnings,
    )?;
    cache.save(&cache_path)?;

    let stem = script.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dep_names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
    let name = project::sanitize_name(&stem, &dep_names);
    let manifest = manifest::render(&manifest::ManifestInput {
        package_name: &name,
        bin_name: &name,
        script_path: &script,
        edition: header.edition.as_deref().unwrap_or(manifest::DEFAULT_EDITION),
        deps: &deps,
    })?;

    let manifest_path = project_dir.join("Cargo.toml");
    // Only rewrite when it changed, to keep Cargo's freshness checks cheap.
    if std::fs::read_to_string(&manifest_path).ok().as_deref() != Some(manifest.as_str()) {
        std::fs::write(&manifest_path, &manifest)
            .with_context(|| format!("writing {}", manifest_path.display()))?;
    }

    Ok(Prepared { script, project_dir, manifest_path, manifest, bin_name: name, warnings })
}

/// Build the prepared project. Returns cargo's exit code on failure.
pub fn build(p: &Prepared, release: bool) -> Result<std::result::Result<PathBuf, i32>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let target_dir = p.project_dir.join("target");
    let mut cmd = Command::new(cargo);
    cmd.arg("build")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&p.manifest_path)
        .arg("--target-dir")
        .arg(&target_dir)
        // Build from the cache dir so the script directory's own
        // `.cargo/config.toml` or workspace never interferes.
        .current_dir(&p.project_dir)
        // Escape hatch for code that wants the script's location at compile time.
        .env("RUSTRUN_SCRIPT_DIR", p.script.parent().unwrap_or(Path::new("/")))
        .env("RUSTRUN_SCRIPT_PATH", &p.script);
    if release {
        cmd.arg("--release");
    }
    let status = cmd.status().context("failed to run `cargo` (is it installed and on PATH?)")?;
    if !status.success() {
        return Ok(Err(status.code().unwrap_or(101)));
    }
    let profile = if release { "release" } else { "debug" };
    Ok(Ok(target_dir.join(profile).join(format!("{}{}", p.bin_name, std::env::consts::EXE_SUFFIX))))
}

/// Heuristic used only to make a better error message after a failed build.
fn looks_like_it_lacks_main(script: &Path) -> bool {
    std::fs::read_to_string(script).is_ok_and(|s| !s.contains("fn main"))
}

/// Run the script's binary in the *caller's* working directory, so runtime
/// relative paths behave exactly as they would for any other program.
fn exec(bin: &Path, p: &Prepared, args: &[OsString]) -> Result<i32> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .env("RUSTRUN_SCRIPT_PATH", &p.script)
        .env("RUSTRUN_SCRIPT_DIR", p.script.parent().unwrap_or(Path::new("/")));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        Err(err).with_context(|| format!("failed to execute {}", bin.display()))
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().with_context(|| format!("failed to execute {}", bin.display()))?;
        Ok(status.code().unwrap_or(1))
    }
}

/// The whole pipeline. Returns the process exit code.
pub fn run(opts: &Options) -> Result<i32> {
    if opts.clean {
        let script = opts
            .script
            .canonicalize()
            .with_context(|| format!("cannot find script `{}`", opts.script.display()))?;
        let root = opts.cache_dir.clone().unwrap_or_else(project::default_cache_root);
        let dir = project::project_dir(&root, &script);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
        }
        return Ok(0);
    }

    let prepared = prepare(opts)?;
    for w in &prepared.warnings {
        eprintln!("warning: {w}");
    }
    if opts.print_manifest {
        print!("{}", prepared.manifest);
        return Ok(0);
    }
    match build(&prepared, opts.release)? {
        Ok(bin) => exec(&bin, &prepared, &opts.args),
        Err(code) => {
            if looks_like_it_lacks_main(&prepared.script) {
                eprintln!("note: rustrun scripts must define `fn main()`");
            }
            Ok(code)
        }
    }
}
