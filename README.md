# rustrun

Run a standalone Rust file without creating a Cargo project or writing a
`Cargo.toml`.

```rust
#!/usr/bin/env rustrun
// deps: serde[derive], serde_json

use serde::Serialize;

#[derive(Serialize)]
struct Point { x: i32, y: i32 }

fn main() {
    println!("{}", serde_json::to_string(&Point { x: 1, y: 2 }).unwrap());
}
```

```console
$ rustrun point.rs
{"x":1,"y":2}
```

You list dependencies by **name only**. rustrun picks their versions the way
`cargo add` does. The long-term goal is to need no header at all, with crates and
features inferred from the script's `use` statements and macros. This
version is the first step toward that.

## Install

```console
$ cargo install --path .
```

## The header

The header is an **ordinary Rust comment**, so the file stays valid Rust and
is compiled exactly as written. rustrun only reads the leading comment block:
the optional shebang, blank lines, comments, and `#![...]` attributes that come
before the first line of code.

```rust
// deps: rand, regex
// deps: serde[derive, rc]
// deps: tokio@1[macros, rt]
// edition: 2024
```

This asks for the newest suitable versions of `rand` and `regex`, `serde`
with two features turned on, and `tokio` with version requirement `1` plus
features. `edition:` sets the
[Rust edition](https://doc.rust-lang.org/edition-guide/) the script is
compiled with (default: 2024). A header line can't end with a second `//`
comment: everything after `deps:` or `edition:` is read as part of the value.

| Form          | Meaning                                                               |
|---------------|-----------------------------------------------------------------------|
| `name`        | resolve the newest suitable version from crates.io                    |
| `name@REQ`    | use the Cargo requirement `REQ` as-is (`0.8`, `=1.2.3`, `~0.4`, `>=1`) |
| `name[a, b]`  | turn on features `a` and `b`                                          |

- Separate entries with commas. You can repeat `deps:` on several lines, and
  `//`, `//!`, `///` and `/* ... */` comments all work.
- If a crate appears more than once, its features are merged. Two different
  version requirements for the same crate are an error.
- `use proc_macro2` style names are accepted. rustrun tries both `_` and `-`
  and writes the real crates.io name (`proc-macro2`).

### How versions are chosen

For each dependency without `@REQ`, rustrun reads the crate's entry in the
crates.io sparse index (the same data `cargo add` uses). It picks the newest
version that is:

1. not yanked,
2. stable (a pre-release is used only if nothing else has been published),
3. compatible with your `rustc`, based on the crate's `rust-version`. If no
   version qualifies, rustrun uses the newest one and prints a warning.

It also checks the requested features against that version and lists the
available features if one is misspelled.

The resolved version is saved in the script's cache. Later runs need no
network access and stay on that version (along with the cached `Cargo.lock`)
until you pass `--update`.

## Multi-file scripts and relative paths

This is the main difference from rust-script. rust-script copies your script
into a hidden project, so compile-time relative paths stop working. It needs
`include!(concat!(env!("RUST_SCRIPT_BASE_PATH"), "/module.rs"))` to get around
this.

rustrun also generates its `Cargo.toml` in a hidden cache directory, but the
binary target points back at your **original file**:

```toml
[[bin]]
name = "point"
path = "/home/you/scripts/point.rs"
```

rustc compiles the file where it is, so all of these work unchanged, relative
to the script's real location:

```rust
mod helper;                              // ./helper.rs or ./helper/mod.rs
#[path = "../shared/common.rs"] mod common;
include!("generated.rs");
const DATA: &str = include_str!("data/input.txt");
```

The program runs in **your** working directory, so runtime paths such as
`File::open("input.txt")` resolve the way you'd expect. Scripts that are
symlinks are resolved to the real file first. rustrun never writes anything
next to your script.

To find files next to the script **at run time** from any working directory,
use these environment variables:

- `RUSTRUN_SCRIPT_PATH` is the script's absolute path, with symlinks resolved
  (`/home/you/scripts/report.rs`)
- `RUSTRUN_SCRIPT_DIR` is the folder that contains it (`/home/you/scripts`)

They are *not* the working directory. If you run
`cd ~/data && rustrun ~/scripts/report.rs`, the cwd is `~/data`, but
`RUSTRUN_SCRIPT_DIR` is `~/scripts`. They are also set at compile time for
`env!(...)`. Don't use `CARGO_MANIFEST_DIR` to find files: it points at the
hidden cache project.

### Scripts must have `fn main()`

Wrapping top-level code in a generated `main` would mean compiling an edited
**copy** of the script, and that would break the relative imports above. So
rustrun requires a normal `fn main()`.

## Command line

```text
rustrun [OPTIONS] <SCRIPT> [ARGS]...

      --release         build with optimizations
      --print-manifest  print the generated Cargo.toml instead of running
      --update          re-resolve versions of crates without an explicit @REQ
      --clean           delete this script's cached project
      --cache-dir DIR   where generated projects live (env: RUSTRUN_CACHE_DIR)
```

Everything after `SCRIPT` goes to the script, including things that look like
rustrun flags (`rustrun s.rs --release` passes `--release` to `s.rs`). The
script's exit code becomes rustrun's exit code.

Each script gets its own project under `<cache>/projects/<name>-<hash>`,
keyed on its full path. Two `main.rs` files in different directories never
clash. The default cache is your OS cache directory, e.g.
`~/.cache/rustrun` or `~/Library/Caches/rustrun`.

## Development

```console
$ cargo test                               # unit + offline integration tests
$ cargo test --test network -- --ignored   # tests against real crates.io
```

| Where                   | What it covers                                                        |
|-------------------------|-----------------------------------------------------------------------|
| `src/header.rs`         | header grammar, comment styles, shebangs, error messages               |
| `src/resolve.rs`        | index layout, yanked / pre-release / MSRV selection, features, caching |
| `src/manifest.rs`       | generated TOML, path escaping                                          |
| `src/project.rs`        | cache layout, package-name sanitizing                                  |
| `tests/local_imports.rs`| `mod`, `#[path]`, `include!`, `include_str!`, symlinks, cwd semantics, rebuilds |
| `tests/cli.rs`          | args, exit codes, errors, version resolution against a fake on-disk index |
| `tests/network.rs`      | real crates and features from crates.io (`#[ignore]`d by default)     |

The offline tests never touch the network. `RUSTRUN_INDEX` points them at a
fake sparse index written to a temp directory.

## Roadmap

- [x] **First pass:** names in a comment header, versions resolved
      automatically, multi-file scripts compiled in place
- [ ] Find crates from `use` paths and macro invocations with `syn`, so no
      header is needed
- [ ] Infer features (e.g. `#[derive(Serialize)]` means `serde/derive`)
- [ ] Disambiguate crate names (renamed crates, module name ≠ crate name)
- [ ] Local path dependencies, `default-features = false`, multi-comparator
      requirements (`>=1.2, <1.5`, which can't be written in the header yet
      because commas separate entries)
- [ ] Publish on crates.io

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
