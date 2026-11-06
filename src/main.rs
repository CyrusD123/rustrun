use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// Run a standalone Rust script. Dependencies are listed by name in a
/// `// deps: ...` comment; versions are resolved from crates.io.
#[derive(Parser, Debug)]
#[command(name = "rustrun", version)]
struct Cli {
    /// Build with optimizations.
    #[arg(long)]
    release: bool,

    /// Print the generated Cargo.toml and exit without building.
    #[arg(long)]
    print_manifest: bool,

    /// Re-resolve the latest versions of dependencies without an explicit `@version`.
    #[arg(long)]
    update: bool,

    /// Delete this script's cached build project and exit.
    #[arg(long)]
    clean: bool,

    /// Where to keep generated projects [default: the OS cache dir].
    #[arg(long, env = "RUSTRUN_CACHE_DIR", value_name = "DIR")]
    cache_dir: Option<PathBuf>,

    /// Sparse registry index URL (or a local directory laid out like one).
    #[arg(long, env = "RUSTRUN_INDEX", value_name = "URL|DIR", hide = true)]
    index: Option<String>,

    /// The script to run.
    script: PathBuf,

    /// Arguments passed through to the script (everything after SCRIPT).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<OsString>,
}

/// rustrun's options that take a separate value (`--cache-dir DIR`).
const VALUE_OPTIONS: &[&str] = &["--cache-dir", "--index"];

/// Split argv so that everything after the script path belongs to the
/// script, even things that look like rustrun flags (`script.rs --release`).
fn split_argv(argv: Vec<OsString>) -> (Vec<OsString>, Vec<OsString>) {
    let mut i = 1;
    while i < argv.len() {
        let arg = argv[i].to_string_lossy();
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            break;
        }
        if VALUE_OPTIONS.contains(&arg.as_ref()) {
            i += 1;
        }
        i += 1;
    }
    let split = (i + 1).min(argv.len());
    let mut ours = argv;
    let script_args = ours.split_off(split);
    (ours, script_args)
}

fn main() -> ExitCode {
    let (ours, script_args) = split_argv(std::env::args_os().collect());
    let cli = Cli::parse_from(ours);
    let opts = rustrun::Options {
        script: cli.script,
        args: script_args,
        release: cli.release,
        print_manifest: cli.print_manifest,
        update: cli.update,
        clean: cli.clean,
        cache_dir: cli.cache_dir,
        index: cli.index,
    };
    match rustrun::run(&opts) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::split_argv;

    fn split(args: &[&str]) -> (Vec<String>, Vec<String>) {
        let argv = std::iter::once("rustrun").chain(args.iter().copied()).map(Into::into).collect();
        let (a, b) = split_argv(argv);
        let s = |v: Vec<std::ffi::OsString>| v.into_iter().map(|x| x.into_string().unwrap()).collect();
        (s(a), s(b))
    }

    #[test]
    fn flags_after_script_belong_to_script() {
        let (ours, theirs) = split(&["--release", "s.rs", "--release", "-x"]);
        assert_eq!(ours, ["rustrun", "--release", "s.rs"]);
        assert_eq!(theirs, ["--release", "-x"]);
    }

    #[test]
    fn value_options_are_skipped() {
        let (ours, theirs) = split(&["--cache-dir", "c", "s.rs", "a"]);
        assert_eq!(ours, ["rustrun", "--cache-dir", "c", "s.rs"]);
        assert_eq!(theirs, ["a"]);
        let (ours, _) = split(&["--cache-dir=c", "s.rs"]);
        assert_eq!(ours, ["rustrun", "--cache-dir=c", "s.rs"]);
    }

    #[test]
    fn double_dash_and_no_script() {
        let (ours, theirs) = split(&["--", "-weird.rs", "x"]);
        assert_eq!(ours, ["rustrun", "--", "-weird.rs"]);
        assert_eq!(theirs, ["x"]);
        let (ours, theirs) = split(&["--help"]);
        assert_eq!(ours, ["rustrun", "--help"]);
        assert!(theirs.is_empty());
    }
}
