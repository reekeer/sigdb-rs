mod common;

use common::*;
use sigdb::Value;

fn run_dir(dir: &str) {
    let cases = cases(dir);
    assert!(!cases.is_empty(), "no cases in {dir}");
    let mut failures = Vec::new();
    for case in cases {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let data = case_bytes(&case);
        if data != case_bytes(&case) {
            failures.push(format!("{name}: build is not reproducible"));
        }
        let db = sigdb::load_bytes(data).unwrap();
        let expected = read_json(&case.join("sections.json"));
        let digests = digests_value(&db);
        if !py_eq(&digests, &expected) {
            failures.push(format!("{name}: section digests changed: {}", digests.to_json().unwrap()));
        }
        failures.extend(check_vectors(&db, &case, &name));
    }
    assert!(failures.is_empty(), "golden vector mismatch:\n{}", failures.join("\n"));
}

#[test]
fn golden() {
    run_dir("golden");
}

#[test]
fn golden_extra() {
    run_dir("golden-extra");
}

#[test]
fn build_errors() {
    let cases = read_json(&tests_dir().join("golden/build_errors.json"));
    let mut failures = Vec::new();
    for (i, case) in cases.as_array().unwrap().iter().enumerate() {
        let actual = match options_from(case.get("build").unwrap()) {
            Err(e) => Some(e),
            Ok((rules, options)) => sigdb::build_bytes(rules.as_ref(), &options).err(),
        };
        let actual = actual.map_or(Value::Null, |e| {
            Value::from_iter([("error", Value::from(e.kind_name())), ("message", Value::from(e.message()))])
        });
        let expect = case.get("expect").cloned().unwrap_or(Value::Null);
        if !py_eq(&actual, &expect) {
            failures.push(format!(
                "build_errors[{i}] expected {} got {}",
                expect.to_json().unwrap(),
                actual.to_json().unwrap()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
