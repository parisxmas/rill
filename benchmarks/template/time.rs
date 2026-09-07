// The same timing the Rill harness does: clear outside the timed span, one
// clock read on each side of the render, mean and best over N renders.
use std::hint::black_box;
use std::time::Instant;

use tmpls::{Benchmark, BigTable, Output, Teams};

fn go<B: Benchmark>(case: &str, n: u64) {
    let mut this = B::default();
    let mut out = B::Output::default();
    let big = BigTable::default();
    let teams = Teams::default();
    let t0 = Instant::now();
    for _ in 0..n {
        out.clear();
        match case {
            "big-table" => this.big_table(black_box(&mut out), black_box(&big)).unwrap(),
            _ => this.teams(black_box(&mut out), black_box(&teams)).unwrap(),
        }
    }
    let total = t0.elapsed().as_nanos();
    let bytes = out.as_bytes().len();
    println!("{case} {bytes} bytes {n} renders");
    println!("  each {} ns", total / n as u128);
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let case = a[1].as_str();
    let engine = a[2].as_str();
    let n: u64 = a[3].parse().unwrap();
    match engine {
        "sailfish" => go::<sailfish::Benchmark>(case, n),
        "askama" => go::<askama::Benchmark>(case, n),
        "markup" => go::<markup::Benchmark>(case, n),
        "maud" => go::<maud::Benchmark>(case, n),
        _ => panic!("unknown engine"),
    }
}
