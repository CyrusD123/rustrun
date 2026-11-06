//! Compile-time imports across files.
//!
//! These are the cases rust-script needs `RUST_SCRIPT_BASE_PATH` for. Because
//! our generated manifest's `[[bin]] path` is the original file, rustc
//! resolves every one of them relative to the script's real location.

mod common;
use common::*;

#[test]
fn sibling_module_file() {
    let f = Fixture::new();
    f.write("helper.rs", "pub fn greet() -> &'static str { \"hi from helper\" }\n");
    f.write("main.rs", "mod helper;\nfn main() { println!(\"{}\", helper::greet()); }\n");
    assert_eq!(f.run_ok(&["main.rs"]), "hi from helper\n");
}

#[test]
fn module_directory_with_mod_rs() {
    let f = Fixture::new();
    f.write("utils/mod.rs", "pub mod math;\n");
    f.write("utils/math.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");
    f.write("calc.rs", "mod utils;\nfn main() { println!(\"{}\", utils::math::add(2, 3)); }\n");
    assert_eq!(f.run_ok(&["calc.rs"]), "5\n");
}

#[test]
fn non_mod_rs_nested_modules() {
    let f = Fixture::new();
    f.write("a.rs", "pub mod b;\n");
    f.write("a/b.rs", "pub mod c;\n");
    f.write("a/b/c.rs", "pub const DEPTH: u8 = 3;\n");
    f.write("deep.rs", "mod a;\nfn main() { println!(\"{}\", a::b::c::DEPTH); }\n");
    assert_eq!(f.run_ok(&["deep.rs"]), "3\n");
}

#[test]
fn inline_module_with_file_child() {
    let f = Fixture::new();
    f.write("outer/inner.rs", "pub fn f() -> u8 { 42 }\n");
    f.write("s.rs", "mod outer { pub mod inner; }\nfn main() { println!(\"{}\", outer::inner::f()); }\n");
    assert_eq!(f.run_ok(&["s.rs"]), "42\n");
}

#[test]
fn path_attribute() {
    let f = Fixture::new();
    f.write("lib/other_name.rs", "pub fn f() -> &'static str { \"renamed\" }\n");
    f.write(
        "p.rs",
        "#[path = \"lib/other_name.rs\"]\nmod renamed;\nfn main() { println!(\"{}\", renamed::f()); }\n",
    );
    assert_eq!(f.run_ok(&["p.rs"]), "renamed\n");
}

#[test]
fn path_attribute_to_parent_directory() {
    let f = Fixture::new();
    f.write("shared/common.rs", "pub fn f() -> u8 { 7 }\n");
    f.write(
        "bin/tool.rs",
        "#[path = \"../shared/common.rs\"]\nmod common;\nfn main() { println!(\"{}\", common::f()); }\n",
    );
    assert_eq!(f.run_ok(&["bin/tool.rs"]), "7\n");
}

#[test]
fn include_macro_at_item_level() {
    let f = Fixture::new();
    f.write("generated.rs", "fn generated() -> u32 { 1234 }\n");
    f.write("inc.rs", "include!(\"generated.rs\");\nfn main() { println!(\"{}\", generated()); }\n");
    assert_eq!(f.run_ok(&["inc.rs"]), "1234\n");
}

#[test]
fn include_str_and_include_bytes() {
    let f = Fixture::new();
    f.write("data/greeting.txt", "hello, file");
    f.write("data/bytes.bin", "\x01\x02\x03");
    f.write("../sibling_of_root.txt", "from the parent dir");
    f.write(
        "data_user.rs",
        r#"
const GREETING: &str = include_str!("data/greeting.txt");
const BYTES: &[u8] = include_bytes!("data/bytes.bin");
const PARENT: &str = include_str!("../sibling_of_root.txt");
fn main() { println!("{GREETING}|{}|{PARENT}", BYTES.len()); }
"#,
    );
    assert_eq!(f.run_ok(&["data_user.rs"]), "hello, file|3|from the parent dir\n");
}

#[test]
fn include_str_inside_submodule_is_relative_to_that_module() {
    let f = Fixture::new();
    f.write("assets/mod.rs", "pub const TEXT: &str = include_str!(\"text.txt\");\n");
    f.write("assets/text.txt", "module-relative");
    f.write("m.rs", "mod assets;\nfn main() { println!(\"{}\", assets::TEXT); }\n");
    assert_eq!(f.run_ok(&["m.rs"]), "module-relative\n");
}

#[test]
fn compile_time_paths_resolve_from_script_not_cwd() {
    // Run from a completely different directory than the script lives in.
    let f = Fixture::new();
    f.write("project/scripts/helper.rs", "pub const X: &str = include_str!(\"x.txt\");\n");
    f.write("project/scripts/x.txt", "found");
    let script =
        f.write("project/scripts/run.rs", "mod helper;\nfn main() { println!(\"{}\", helper::X); }\n");
    std::fs::create_dir_all(f.path("elsewhere")).unwrap();

    let out = f.cmd().current_dir(f.path("elsewhere")).arg(&script).output().unwrap();
    assert_success(&out);
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "found\n");

    // Relative script paths work from a different cwd too.
    let out = f.cmd().current_dir(f.path("project")).arg("scripts/run.rs").output().unwrap();
    assert_success(&out);
}

#[test]
fn runtime_relative_paths_use_callers_cwd() {
    // Unlike compile-time paths, runtime file access is relative to the
    // directory the user ran the command from: we never chdir into the cache.
    let f = Fixture::new();
    f.write("work/input.txt", "runtime data");
    let script = f.write(
        "scripts/read.rs",
        r#"fn main() {
    println!("{}", std::fs::read_to_string("input.txt").unwrap());
    println!("{}", std::env::current_dir().unwrap().display());
}"#,
    );
    let work = f.path("work").canonicalize().unwrap();
    let out = f.cmd().current_dir(&work).arg(&script).output().unwrap();
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).unwrap();
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some("runtime data"));
    assert_eq!(lines.next(), Some(work.to_str().unwrap()));
}

#[test]
fn script_location_env_vars() {
    let f = Fixture::new();
    let script = f.write(
        "dir/where.rs",
        r#"fn main() {
    println!("{}", std::env::var("RUSTRUN_SCRIPT_PATH").unwrap());
    println!("{}", std::env::var("RUSTRUN_SCRIPT_DIR").unwrap());
    println!("{}", env!("RUSTRUN_SCRIPT_DIR"));
}"#,
    );
    let canon = script.canonicalize().unwrap();
    let dir = canon.parent().unwrap().to_str().unwrap().to_string();
    let out = f.run_ok(&[script.to_str().unwrap()]);
    assert_eq!(out, format!("{}\n{dir}\n{dir}\n", canon.display()));
}

#[test]
fn directories_with_spaces_and_unicode() {
    let f = Fixture::new();
    f.write(
        "my scripts/ünïcødé dir/helper.rs",
        "pub fn f() -> &'static str { include_str!(\"data file.txt\") }\n",
    );
    f.write("my scripts/ünïcødé dir/data file.txt", "ok");
    f.write(
        "my scripts/ünïcødé dir/my script.rs",
        "mod helper;\nfn main() { println!(\"{}\", helper::f()); }\n",
    );
    assert_eq!(f.run_ok(&["my scripts/ünïcødé dir/my script.rs"]), "ok\n");
}

#[cfg(unix)]
#[test]
fn symlinked_script_resolves_modules_next_to_real_file() {
    let f = Fixture::new();
    f.write("real/helper.rs", "pub const WHERE: &str = \"real dir\";\n");
    let real = f.write("real/tool.rs", "mod helper;\nfn main() { println!(\"{}\", helper::WHERE); }\n");
    std::fs::create_dir_all(f.path("bin")).unwrap();
    std::os::unix::fs::symlink(&real, f.path("bin/tool.rs")).unwrap();
    assert_eq!(f.run_ok(&["bin/tool.rs"]), "real dir\n");
}

#[test]
fn script_directory_is_left_untouched() {
    // No Cargo.toml, target/, Cargo.lock or copied sources next to the script.
    let f = Fixture::new();
    f.write("helper.rs", "pub fn f() {}\n");
    f.write("clean.rs", "mod helper;\nfn main() { helper::f(); }\n");
    let before = snapshot(&f.base());
    f.run_ok(&["clean.rs"]);
    assert_eq!(snapshot(&f.base()), before);
}

#[test]
fn unrelated_cargo_project_around_script_does_not_interfere() {
    // A broken Cargo.toml / workspace next to the script must be ignored:
    // the build happens in the cache project, which is its own workspace.
    let f = Fixture::new();
    f.write("Cargo.toml", "this is [not valid toml\n");
    f.write(".cargo/config.toml", "[build]\ntarget = \"no-such-target-triple\"\n");
    f.write("loose.rs", "fn main() { println!(\"fine\"); }\n");
    assert_eq!(f.run_ok(&["loose.rs"]), "fine\n");
}

#[test]
fn editing_a_module_triggers_rebuild() {
    let f = Fixture::new();
    f.write("helper.rs", "pub const V: u8 = 1;\n");
    f.write("main.rs", "mod helper;\nfn main() { println!(\"{}\", helper::V); }\n");
    assert_eq!(f.run_ok(&["main.rs"]), "1\n");
    // Ensure a distinct mtime on coarse-grained filesystems.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    f.write("helper.rs", "pub const V: u8 = 2;\n");
    assert_eq!(f.run_ok(&["main.rs"]), "2\n");
}

#[test]
fn editing_an_included_data_file_triggers_rebuild() {
    let f = Fixture::new();
    f.write("msg.txt", "first");
    f.write("show.rs", "fn main() { println!(\"{}\", include_str!(\"msg.txt\")); }\n");
    assert_eq!(f.run_ok(&["show.rs"]), "first\n");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    f.write("msg.txt", "second");
    assert_eq!(f.run_ok(&["show.rs"]), "second\n");
}
