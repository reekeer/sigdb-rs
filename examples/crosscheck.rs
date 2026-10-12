#[path = "../tests/common/mod.rs"]
mod common;

use std::path::PathBuf;

use common::{check_vectors, options_from, read_json};
use sigdb::Value;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: crosscheck <cases dir>"));
    let mut cases: Vec<PathBuf> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    cases.sort();
    let (mut failures, mut built, mut errors, mut vectors) = (Vec::new(), 0, 0, 0);
    for case in &cases {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let build = read_json(&case.join("build.json"));
        let result = options_from(&build).and_then(|(rules, opts)| sigdb::build_bytes(rules.as_ref(), &opts));
        let expected_error = case.join("error.json");
        if expected_error.is_file() {
            errors += 1;
            let expect = read_json(&expected_error);
            match result {
                Ok(_) => failures.push(format!("{name}: python failed with {expect}, rust built a file")),
                Err(e) => {
                    let actual = Value::from_iter([
                        ("error", Value::from(e.kind_name())),
                        ("message", Value::from(e.message())),
                    ]);
                    if !common::py_eq(&actual, &expect) {
                        failures.push(format!("{name}: error {actual} != python {expect}"));
                    }
                }
            }
            continue;
        }
        built += 1;
        let data = match result {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{name}: rust build failed: {e}"));
                continue;
            }
        };
        let py = std::fs::read(case.join("py.sigdb")).unwrap();
        if data != py {
            let a = sigdb::load_bytes(data.clone()).map(|d| d.section_digests());
            let b = sigdb::load_bytes(py.clone()).map(|d| d.section_digests());
            failures.push(format!("{name}: file bytes differ (digests equal: {})", a == b));
        }
        match std::fs::read(case.join("rs.sigdb")) {
            Ok(earlier) if earlier != data => {
                failures.push(format!("{name}: rs.sigdb differs from this build"))
            }
            Ok(_) => {}
            Err(_) => std::fs::write(case.join("rs.sigdb"), &data).unwrap(),
        }
        let n = read_json(&case.join("vectors.json")).as_array().map_or(0, Vec::len);
        vectors += n;
        let mut files = vec![("py", py), ("rs", data)];
        if let Ok(ts) = std::fs::read(case.join("ts.sigdb")) {
            files.push(("ts", ts));
        }
        for (label, bytes) in files {
            match sigdb::load_bytes(bytes) {
                Ok(db) => failures.extend(check_vectors(&db, case, &format!("{name}/{label}"))),
                Err(e) => failures.push(format!("{name}/{label}: load failed: {e}")),
            }
        }
    }
    println!("{} cases: {built} built, {errors} build errors, {vectors} vectors", cases.len());
    if !failures.is_empty() {
        for f in failures.iter().take(40) {
            eprintln!("{f}");
        }
        eprintln!("{} mismatches", failures.len());
        std::process::exit(1);
    }
    println!("all identical");
}
