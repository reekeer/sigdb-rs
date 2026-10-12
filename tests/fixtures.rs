mod common;

use common::*;
use sigdb::{LoadOptions, Value};

fn check_fixture_dir(sub: &str) {
    let dir = tests_dir().join("fixtures").join(sub);
    let mut fixtures: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "sigdb"))
        .collect();
    fixtures.sort();
    assert!(!fixtures.is_empty(), "no fixtures in {}", dir.display());
    let mut failures = Vec::new();
    for fixture in fixtures {
        let stem = fixture.file_stem().unwrap().to_string_lossy().to_string();
        let case = ["golden", "golden-extra"]
            .iter()
            .map(|g| tests_dir().join(g).join(&stem))
            .find(|p| p.is_dir())
            .unwrap_or_else(|| panic!("no golden case for {stem}"));
        let db = sigdb::load_with(&fixture, &LoadOptions { lazy: false, ..LoadOptions::default() }).unwrap();
        let expected = read_json(&case.join("sections.json"));
        if !py_eq(&digests_value(&db), &expected) {
            failures.push(format!("{sub}/{stem}: section digests differ"));
        }
        failures.extend(check_vectors(&db, &case, &format!("{sub}/{stem}")));
        let rebuilt = case_bytes(&case);
        let rebuilt_db = sigdb::load_bytes(rebuilt.clone()).unwrap();
        if rebuilt_db.section_digests() != db.section_digests() {
            failures.push(format!("{sub}/{stem}: rust build differs from the fixture"));
        }
        if sub == "py" && rebuilt != std::fs::read(&fixture).unwrap() {
            failures.push(format!("{sub}/{stem}: rust file bytes differ from the python file"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn python_fixtures() {
    check_fixture_dir("py");
}

#[test]
fn typescript_fixtures() {
    check_fixture_dir("ts");
}

#[test]
fn html_heads_reference() {
    let cases = read_json(&tests_dir().join("fixtures/html_heads.json"));
    let cases = cases.as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let html = case.get("html").and_then(Value::as_str).unwrap();
        let expected: Vec<&str> =
            case.get("heads").unwrap().as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(sigdb::html::html_heads(html), expected, "{html:?}");
    }
}

#[test]
fn repr_reference() {
    let cases = read_json(&tests_dir().join("fixtures/repr.json"));
    for case in cases.as_array().unwrap() {
        let expected = case.get("repr").and_then(Value::as_str).unwrap();
        let actual = match case.get("value") {
            Some(Value::String(s)) => sigdb::text::repr_str(s),
            _ => {
                let list: Vec<&str> = case
                    .get("list")
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap())
                    .collect();
                sigdb::text::repr_list(&list)
            }
        };
        assert_eq!(actual, expected);
    }
}
