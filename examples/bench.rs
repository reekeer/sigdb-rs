//! Load and query timings on a real file.
//!
//!     cargo run --release --example bench -- <file.sigdb>

use std::time::Instant;

use sigdb::{LoadOptions, load_with};

fn main() {
    let path = std::env::args().nth(1).expect("file.sigdb");
    let t = Instant::now();
    let db = load_with(&path, &LoadOptions::default()).unwrap();
    let parsed = t.elapsed();
    let t = Instant::now();
    for name in db.index_names() {
        db.index(name).unwrap().preload().unwrap();
    }
    println!(
        "parse container {:.1} ms, decompress+parse indexes {:.0} ms",
        parsed.as_secs_f64() * 1e3,
        t.elapsed().as_secs_f64() * 1e3
    );
    let main = db.index("main").unwrap();
    let step = (main.pattern_total() / 300).max(1);
    let tokens: Vec<&str> =
        (0..main.pattern_total()).step_by(step).map(|i| main.pattern_text(i).unwrap()).collect();
    if let Some((group, _)) =
        main.groups.iter().find(|(_, s)| s.match_mode == sigdb::MatchMode::Exact && s.name != "headers")
    {
        let t = Instant::now();
        let n = 1000;
        let mut total = 0;
        for _ in 0..n {
            total += main.match_tokens(group, tokens.as_slice()).unwrap().len();
        }
        println!(
            "match_tokens {} tokens: {:.3} ms/call ({} items)",
            tokens.len(),
            t.elapsed().as_secs_f64() * 1e3 / n as f64,
            total / n
        );
    }

    // Aho-Corasick: 25k "contains" patterns taken from the file, scanned over 1 MB of text made
    // of them (the same shape as the Python benchmark in the README).
    let patterns: Vec<&str> =
        (0..main.pattern_total()).step_by(6).take(25_000).map(|i| main.pattern_text(i).unwrap()).collect();
    let rules: sigdb::Value = patterns
        .iter()
        .enumerate()
        .map(|(i, p)| (format!("r{i}"), sigdb::Value::from_iter([("strings", sigdb::Value::from(vec![*p]))])))
        .collect();
    let options = sigdb::BuildOptions::new()
        .groups(sigdb::Value::from_iter([("strings", sigdb::Value::from_iter([("match", "contains")]))]));
    let t = Instant::now();
    let data = sigdb::build_bytes(Some(&rules), &options).unwrap();
    println!("build {} contains patterns: {:.2} s", patterns.len(), t.elapsed().as_secs_f64());
    let db = sigdb::load_bytes(data).unwrap();
    db.main().unwrap().preload().unwrap();
    let mut seed: u64 = 1;
    let mut text = String::new();
    while text.len() < 1_000_000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        text.push_str(patterns[(seed >> 33) as usize % patterns.len()]);
        text.push(' ');
    }
    let text: String = text.chars().take(1_000_000).collect();
    let t = Instant::now();
    let occ = db.scan(&text, "strings").unwrap();
    let dt = t.elapsed().as_secs_f64();
    println!(
        "scan {:.2} MB: {:.1} ms ({:.0} MB/s), {} occurrences",
        text.len() as f64 / 1e6,
        dt * 1e3,
        text.len() as f64 / dt / 1e6,
        occ.len()
    );
}
