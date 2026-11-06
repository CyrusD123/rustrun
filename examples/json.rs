#!/usr/bin/env rustrun
// deps: serde[derive], serde_json
//
// Versions are resolved from crates.io on first run and then remembered.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
struct Config {
    name: String,
    retries: u32,
}

fn main() {
    let cfg: Config = serde_json::from_str(r#"{"name":"demo","retries":3}"#).unwrap();
    println!("{cfg:?}");
    println!("{}", serde_json::to_string_pretty(&cfg).unwrap());
}
