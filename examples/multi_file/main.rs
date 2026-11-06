#!/usr/bin/env rustrun
// A script split across several files. Nothing here needs environment
// variables or special paths: rustc compiles this file in place.

mod geometry;

const SHAPES: &str = include_str!("data/shapes.txt");

fn main() {
    for line in SHAPES.lines() {
        let side: f64 = line.trim().parse().unwrap();
        println!("square of side {side}: area {}", geometry::square::area(side));
    }
}
