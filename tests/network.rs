//! Tests that resolve against the real crates.io index and download crates.
//! Ignored by default; run with `cargo test -- --ignored`.

mod common;
use common::*;

fn online(f: &Fixture, args: &[&str]) -> String {
    let out = f.cmd().env_remove("RUSTRUN_INDEX").args(args).output().unwrap();
    assert_success(&out);
    String::from_utf8(out.stdout).unwrap()
}

#[test]
#[ignore = "needs network access"]
fn real_dependency_without_version() {
    let f = Fixture::new();
    f.write(
        "fmt.rs",
        "// deps: itoa\nfn main() { let mut b = itoa::Buffer::new(); println!(\"{}\", b.format(42u8)); }\n",
    );
    assert_eq!(online(&f, &["fmt.rs"]), "42\n");
}

#[test]
#[ignore = "needs network access"]
fn real_dependency_with_features() {
    let f = Fixture::new();
    f.write(
        "json.rs",
        r##"// deps: serde[derive], serde_json
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
struct Point { x: i32, y: i32 }

fn main() {
    let p: Point = serde_json::from_str(r#"{"x":1,"y":2}"#).unwrap();
    println!("{}", serde_json::to_string(&p).unwrap());
}
"##,
    );
    assert_eq!(online(&f, &["json.rs"]), "{\"x\":1,\"y\":2}\n");
}

#[test]
#[ignore = "needs network access"]
fn real_dependency_with_local_module() {
    // Dependencies and local modules together: the module uses the crate.
    let f = Fixture::new();
    f.write("util.rs", "pub fn fmt(n: u64) -> String { itoa::Buffer::new().format(n).to_string() }\n");
    f.write("both.rs", "// deps: itoa\nmod util;\nfn main() { println!(\"{}\", util::fmt(7)); }\n");
    assert_eq!(online(&f, &["both.rs"]), "7\n");
}

#[test]
#[ignore = "needs network access"]
fn underscore_name_resolves_to_hyphenated_crate() {
    let f = Fixture::new();
    f.write("pm.rs", "// deps: proc_macro2\nfn main() { println!(\"{}\", proc_macro2::Span::call_site().start().line); }\n");
    let out = f.cmd().env_remove("RUSTRUN_INDEX").args(["--print-manifest", "pm.rs"]).output().unwrap();
    assert_success(&out);
    assert!(String::from_utf8(out.stdout).unwrap().contains("proc-macro2 = \""));
}
