//! End-to-end behaviour of the `rustrun` binary that does not need network
//! access. Version resolution is exercised against a fake sparse index laid
//! out on disk, with `--print-manifest` so nothing is downloaded.

mod common;
use common::*;

fn manifest(f: &Fixture, script: &str) -> toml::Table {
    let out =
        f.cmd().env("RUSTRUN_INDEX", f.path("../index")).args(["--print-manifest", script]).output().unwrap();
    assert_success(&out);
    toml::from_str(&String::from_utf8(out.stdout).unwrap()).unwrap()
}

fn fake_index(f: &Fixture) {
    write_index(
        &f.path("../index"),
        &[
            ("itoa", &[("1.0.0", Some("1.36"), &[]), ("1.0.18", Some("1.68"), &[])]),
            ("serde", &[("1.0.200", None, &["derive", "std"]), ("2.0.0-alpha.1", None, &["derive"])]),
            ("proc-macro2", &[("1.0.90", None, &[])]),
            ("future-only", &[("0.1.0", Some("1.40"), &[]), ("0.2.0", Some("9.99"), &[])]),
        ],
    );
}

// ---------------------------------------------------------------- running

#[test]
fn runs_a_plain_script() {
    let f = Fixture::new();
    f.write("hello.rs", "fn main() { println!(\"Hello, world!\"); }\n");
    assert_eq!(f.run_ok(&["hello.rs"]), "Hello, world!\n");
}

#[test]
fn shebang_line_is_accepted_unmodified() {
    let f = Fixture::new();
    f.write("sb.rs", "#!/usr/bin/env rustrun\n// deps:\nfn main() { println!(\"shebang ok\"); }\n");
    assert_eq!(f.run_ok(&["sb.rs"]), "shebang ok\n");
}

#[test]
fn arguments_pass_through_including_flag_lookalikes() {
    let f = Fixture::new();
    f.write("args.rs", "fn main() { println!(\"{:?}\", std::env::args().skip(1).collect::<Vec<_>>()); }\n");
    assert_eq!(
        f.run_ok(&["args.rs", "a", "--release", "-x", "--", "b c"]),
        "[\"a\", \"--release\", \"-x\", \"--\", \"b c\"]\n"
    );
}

#[test]
fn exit_code_propagates() {
    let f = Fixture::new();
    f.write("exit.rs", "fn main() { std::process::exit(7); }\n");
    assert_eq!(f.run(&["exit.rs"]).status.code(), Some(7));
}

#[test]
fn stdin_is_inherited() {
    use std::io::Write;
    let f = Fixture::new();
    f.write("cat.rs", "use std::io::Read;\nfn main() { let mut s = String::new(); std::io::stdin().read_to_string(&mut s).unwrap(); print!(\"{}\", s.to_uppercase()); }\n");
    let mut child = f
        .cmd()
        .arg("cat.rs")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"piped").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_success(&out);
    assert_eq!(out.stdout, b"PIPED");
}

#[test]
fn compile_errors_are_reported_with_original_path() {
    let f = Fixture::new();
    f.write("broken.rs", "fn main() { let x: u32 = \"nope\"; }\n");
    let out = f.run(&["broken.rs"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("mismatched types"), "{err}");
    // Diagnostics point at the user's real file, not a copy.
    let real = f.path("broken.rs").canonicalize().unwrap();
    assert!(err.contains(real.to_str().unwrap()), "{err}");
}

#[test]
fn missing_main_gets_a_hint() {
    let f = Fixture::new();
    f.write("nomain.rs", "println!(\"top-level code\");\n");
    let out = f.run(&["nomain.rs"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("must define `fn main()`"), "{}", stderr(&out));
}

#[test]
fn missing_script_is_a_clear_error() {
    let f = Fixture::new();
    let out = f.run(&["does_not_exist.rs"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("cannot find script `does_not_exist.rs`"), "{}", stderr(&out));
}

#[test]
fn directory_is_not_a_script() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.path("adir")).unwrap();
    assert!(stderr(&f.run(&["adir"])).contains("is not a file"));
}

#[test]
fn awkward_file_names_build() {
    let f = Fixture::new();
    for name in
        ["test.rs", "fn.rs", "123-start.rs", "my.script.rs", "deps.rs", "build.rs", "UPPER.rs", "noext"]
    {
        f.write(name, &format!("fn main() {{ println!(\"{name}\"); }}\n"));
        assert_eq!(f.run_ok(&[name]), format!("{name}\n"), "script {name}");
    }
}

#[test]
fn same_file_name_in_different_directories_does_not_collide() {
    let f = Fixture::new();
    f.write("a/main.rs", "fn main() { println!(\"A\"); }\n");
    f.write("b/main.rs", "fn main() { println!(\"B\"); }\n");
    for _ in 0..2 {
        assert_eq!(f.run_ok(&["a/main.rs"]), "A\n");
        assert_eq!(f.run_ok(&["b/main.rs"]), "B\n");
    }
    assert_eq!(f.projects().len(), 2);
}

#[test]
fn editing_the_script_triggers_rebuild() {
    let f = Fixture::new();
    f.write("edit.rs", "fn main() { println!(\"v1\"); }\n");
    assert_eq!(f.run_ok(&["edit.rs"]), "v1\n");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    f.write("edit.rs", "fn main() { println!(\"v2\"); }\n");
    assert_eq!(f.run_ok(&["edit.rs"]), "v2\n");
}

#[test]
fn edition_header() {
    let f = Fixture::new();
    // `gen` is a reserved keyword only in edition 2024.
    f.write("ed.rs", "// edition: 2021\nfn main() { let gen = 1; println!(\"{gen}\"); }\n");
    assert_eq!(f.run_ok(&["ed.rs"]), "1\n");
    f.write("ed24.rs", "// edition: 2024\nfn main() { let gen = 1; println!(\"{gen}\"); }\n");
    assert!(!f.run(&["ed24.rs"]).status.success());
}

#[test]
fn default_edition_is_2024() {
    let f = Fixture::new();
    // With no `edition:` line, `gen` is reserved (2024) and let-chains work (2024-only).
    f.write("gen.rs", "fn main() { let gen = 1; println!(\"{gen}\"); }\n");
    assert!(!f.run(&["gen.rs"]).status.success());
    f.write(
        "chains.rs",
        "fn main() { let x = Some(3); if let Some(n) = x && n > 2 { println!(\"chained {n}\"); } }\n",
    );
    assert_eq!(f.run_ok(&["chains.rs"]), "chained 3\n");
}

#[test]
fn release_flag() {
    let f = Fixture::new();
    f.write("rel.rs", "fn main() { println!(\"{}\", cfg!(debug_assertions)); }\n");
    assert_eq!(f.run_ok(&["rel.rs"]), "true\n");
    assert_eq!(f.run_ok(&["--release", "rel.rs"]), "false\n");
}

#[test]
fn clean_removes_cached_project() {
    let f = Fixture::new();
    f.write("c.rs", "fn main() {}\n");
    f.run_ok(&["c.rs"]);
    assert_eq!(f.projects().len(), 1);
    f.run_ok(&["--clean", "c.rs"]);
    assert!(f.projects().is_empty());
}

// ---------------------------------------------------------------- manifest / versions

#[test]
fn manifest_points_at_original_script() {
    let f = Fixture::new();
    fake_index(&f);
    f.write("sub dir/tool.rs", "fn main() {}\n");
    let m = manifest(&f, "sub dir/tool.rs");
    let real = f.path("sub dir/tool.rs").canonicalize().unwrap();
    assert_eq!(m["bin"][0]["path"].as_str(), Some(real.to_str().unwrap()));
    assert_eq!(m["package"]["edition"].as_str(), Some("2024"));
}

#[test]
fn versions_are_auto_derived() {
    let f = Fixture::new();
    fake_index(&f);
    f.write("v.rs", "// deps: itoa, serde[derive], proc_macro2\nfn main() {}\n");
    let m = manifest(&f, "v.rs");
    let deps = m["dependencies"].as_table().unwrap();
    assert_eq!(deps["itoa"].as_str(), Some("1.0.18"));
    // Pre-releases are skipped in favour of the newest stable release.
    assert_eq!(deps["serde"]["version"].as_str(), Some("1.0.200"));
    assert_eq!(deps["serde"]["features"][0].as_str(), Some("derive"));
    // `_`/`-` are normalised to the real crates.io name.
    assert_eq!(deps["proc-macro2"].as_str(), Some("1.0.90"));
}

#[test]
fn rust_version_is_respected() {
    let f = Fixture::new();
    fake_index(&f);
    f.write("msrv.rs", "// deps: future-only\nfn main() {}\n");
    let m = manifest(&f, "msrv.rs");
    assert_eq!(m["dependencies"]["future-only"].as_str(), Some("0.1.0"));
}

#[test]
fn explicit_versions_are_used_verbatim_without_the_index() {
    let f = Fixture::new(); // note: no fake index written at all
    f.write("pin.rs", "// deps: rand@0.8, tokio@1[macros, rt]\nfn main() {}\n");
    let m = manifest(&f, "pin.rs");
    assert_eq!(m["dependencies"]["rand"].as_str(), Some("0.8"));
    assert_eq!(m["dependencies"]["tokio"]["version"].as_str(), Some("1"));
}

#[test]
fn resolved_versions_are_remembered() {
    let f = Fixture::new();
    fake_index(&f);
    f.write("mem.rs", "// deps: itoa\nfn main() {}\n");
    manifest(&f, "mem.rs");

    // A newer release appears on the index, but the script stays pinned...
    write_index(&f.path("../index"), &[("itoa", &[("1.0.18", None, &[]), ("1.1.0", None, &[])])]);
    assert_eq!(manifest(&f, "mem.rs")["dependencies"]["itoa"].as_str(), Some("1.0.18"));

    // ...even with the index gone entirely (offline)...
    std::fs::remove_dir_all(f.path("../index")).unwrap();
    assert_eq!(manifest(&f, "mem.rs")["dependencies"]["itoa"].as_str(), Some("1.0.18"));

    // ...until the user asks for `--update`.
    write_index(&f.path("../index"), &[("itoa", &[("1.0.18", None, &[]), ("1.1.0", None, &[])])]);
    let out = f
        .cmd()
        .env("RUSTRUN_INDEX", f.path("../index"))
        .args(["--update", "--print-manifest", "mem.rs"])
        .output()
        .unwrap();
    assert_success(&out);
    assert!(String::from_utf8(out.stdout).unwrap().contains("itoa = \"1.1.0\""));
}

#[test]
fn unknown_crate_is_reported() {
    let f = Fixture::new();
    fake_index(&f);
    f.write("u.rs", "// deps: definitely-not-a-crate\nfn main() {}\n");
    let out = f.cmd().env("RUSTRUN_INDEX", f.path("../index")).arg("u.rs").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("could not find crate `definitely-not-a-crate`"), "{}", stderr(&out));
}

#[test]
fn unknown_feature_is_reported_with_alternatives() {
    let f = Fixture::new();
    fake_index(&f);
    f.write("feat.rs", "// deps: serde[derrive]\nfn main() {}\n");
    let out = f.cmd().env("RUSTRUN_INDEX", f.path("../index")).arg("feat.rs").output().unwrap();
    let err = stderr(&out);
    assert!(err.contains("does not have feature `derrive`") && err.contains("`derive`"), "{err}");
}

#[test]
fn header_errors_point_at_the_line() {
    let f = Fixture::new();
    f.write("bad.rs", "// a script\n// deps: serde rand\nfn main() {}\n");
    let err = stderr(&f.run(&["bad.rs"]));
    assert!(err.contains("line 2") && err.contains("separate dependencies with commas"), "{err}");
}

#[test]
fn script_named_like_its_dependency() {
    let f = Fixture::new();
    fake_index(&f);
    f.write("itoa.rs", "// deps: itoa\nfn main() {}\n");
    let m = manifest(&f, "itoa.rs");
    assert_eq!(m["bin"][0]["name"].as_str(), Some("itoa_script"));
}
