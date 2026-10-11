//! Shared harness: runs golden vectors the way `tests/test_golden.py` of the Python package does.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use sigdb::{BuildOptions, Database, Error, Hit, MatchResult, Section, Value};

pub fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

pub fn read_json(path: &Path) -> Value {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    sigdb::json::parse_bytes(&bytes).unwrap_or_else(|e| panic!("{}: {e:?}", path.display()))
}

/// Case directories (with `rules.json` or `build.json`) of a golden directory.
pub fn cases(dir: &str) -> Vec<PathBuf> {
    let root = tests_dir().join(dir);
    let mut out: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("rules.json").is_file() || p.join("build.json").is_file())
        .collect();
    out.sort();
    out
}

fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// `(rules, options)` from a `build` object of the golden files.
pub fn options_from(build: &Value) -> Result<(Option<Value>, BuildOptions), Error> {
    let obj = build.as_object().expect("build must be an object");
    let mut options = BuildOptions::new();
    options.groups = obj.get("groups").cloned();
    options.indexes = obj.get("indexes").cloned();
    options.metadata = obj.get("metadata").cloned();
    if let Some(ts) = obj.get("timestamp") {
        match ts {
            Value::Null => {}
            Value::Int(i) => options.timestamp = Some(*i),
            _ => return Err(Error::Format("timestamp must be a non-negative integer".into())),
        }
    }
    if let Some(Value::Object(sections)) = obj.get("sections") {
        for (name, value) in sections {
            let section = match value {
                Value::Object(m) if m.len() == 1 && m.contains_key("hex") => {
                    Section::Blob(hex_bytes(m["hex"].as_str().unwrap()))
                }
                other => Section::Json(other.clone()),
            };
            options.sections.insert(name.clone(), section);
        }
    }
    Ok((obj.get("rules").cloned(), options))
}

pub fn case_build(case: &Path) -> (Option<Value>, BuildOptions) {
    let build = case.join("build.json");
    let value = if build.is_file() {
        read_json(&build)
    } else {
        let mut m = sigdb::Map::new();
        m.insert("rules".into(), read_json(&case.join("rules.json")));
        Value::Object(m)
    };
    options_from(&value).unwrap()
}

pub fn case_bytes(case: &Path) -> Vec<u8> {
    let (rules, options) = case_build(case);
    sigdb::build_bytes(rules.as_ref(), &options).unwrap()
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn opt_usize(v: Option<usize>) -> Value {
    v.map_or(Value::Null, Value::from)
}

fn result(r: &MatchResult<'_>) -> Value {
    obj(vec![
        ("result", r.result.into()),
        ("item_id", opt_usize(r.item_id)),
        ("key", r.item.map_or(Value::Null, |i| i.key.clone().into())),
        ("head", r.head.clone().into()),
        ("pattern_id", opt_usize(r.pattern_id)),
    ])
}

fn hits(hits: &[Hit<'_>]) -> Value {
    Value::Array(
        hits.iter()
            .map(|h| {
                Value::Array(vec![
                    h.item_id.into(),
                    h.item.key.clone().into(),
                    h.hits.into(),
                    Value::Array(h.pattern_ids.iter().map(|&p| p.into()).collect()),
                ])
            })
            .collect(),
    )
}

fn error_value(e: &Error) -> Value {
    obj(vec![("error", e.kind_name().into()), ("message", e.message().into())])
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_else(|| panic!("vector field {k}"))
}

/// One golden vector against a database: the `_run` of `test_golden.py`.
pub fn run_vector(db: &Database, vector: &Value) -> Value {
    match run_inner(db, vector) {
        Ok(v) => v,
        Err(e) => error_value(&e),
    }
}

fn run_inner(db: &Database, vector: &Value) -> Result<Value, Error> {
    let call = s(vector, "call");
    let index = db.index(vector.get("index").and_then(Value::as_str).unwrap_or("main"))?;
    Ok(match call {
        "match" => {
            let head = s(vector, "head");
            let mut out = result(&index.r#match(head)?);
            let all = hits(&index.match_all("headers", head)?);
            out.as_object_mut().unwrap().insert("all".into(), all);
            out
        }
        "match_group" => {
            let group = s(vector, "group");
            let value = s(vector, "value");
            let name = vector.get("name").and_then(Value::as_str);
            let mut out = result(&index.match_group(group, value, name)?);
            let all = match name {
                Some(n) if !n.is_empty() => index.match_all(group, &[(n, value)][..])?,
                _ => index.match_all(group, &[value][..])?,
            };
            out.as_object_mut().unwrap().insert("all".into(), hits(&all));
            out
        }
        "match_html" => result(&index.match_html(s(vector, "html"))?),
        "match_search" => result(&index.match_search(vector.get("search").unwrap())?),
        "match_all" => hits(&index.match_all(s(vector, "group"), vector.get("values").unwrap())?),
        "match_tokens" => {
            let counts = index.match_tokens(s(vector, "group"), vector.get("tokens").unwrap())?;
            Value::Array(counts.iter().map(|(&i, &c)| Value::Array(vec![i.into(), c.into()])).collect())
        }
        "scan" => Value::Array(
            index
                .scan(s(vector, "text"), s(vector, "group"))?
                .iter()
                .map(|o| {
                    Value::Array(vec![
                        o.pattern_id.into(),
                        o.start.into(),
                        o.end.into(),
                        index.pattern_text(o.pattern_id).unwrap().into(),
                    ])
                })
                .collect(),
        ),
        "item" => {
            let key = s(vector, "key");
            match index.item(key) {
                None => Value::Null,
                Some(item) => Value::Array(vec![opt_usize(index.item_id(key)), item.data.clone()]),
            }
        }
        "items_with_prefix" => Value::Array(
            index.items_with_prefix(s(vector, "prefix")).iter().map(|i| i.key.clone().into()).collect(),
        ),
        "pattern_count" => {
            let item_id = vector.get("item_id").and_then(Value::as_i64).unwrap() as usize;
            let group = vector.get("group").and_then(Value::as_str);
            index.pattern_count(item_id, group).unwrap().into()
        }
        "section" => match db.section(s(vector, "name"))? {
            Section::Json(v) => v.clone(),
            Section::Blob(b) => {
                let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
                obj(vec![("hex", hex.into())])
            }
        },
        other => panic!("unknown call: {other}"),
    })
}

/// Python `==`: numbers compare by value (`1 == 1.0 == True`), dicts ignore key order.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Bool(b) => Some(*b as i64 as f64),
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }
    fn big_eq_float(digits: &str, f: f64) -> bool {
        f.is_finite() && f.fract() == 0.0 && format!("{f:.0}") == digits
    }
    match (a, b) {
        (Value::BigInt(x), Value::BigInt(y)) => x == y,
        (Value::BigInt(x), Value::Float(f)) | (Value::Float(f), Value::BigInt(x)) => big_eq_float(x, *f),
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Null, Value::Null) => true,
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| py_eq(v, w)))
        }
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

pub fn digests_value(db: &Database) -> Value {
    Value::Object(db.section_digests().into_iter().map(|(k, v)| (k, Value::String(v))).collect())
}

/// Runs every vector of a case; returns mismatch descriptions.
pub fn check_vectors(db: &Database, case: &Path, label: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let vectors = read_json(&case.join("vectors.json"));
    for (i, vector) in vectors.as_array().unwrap().iter().enumerate() {
        let actual = run_vector(db, vector);
        let expect = vector.get("expect").cloned().unwrap_or(Value::Null);
        if !py_eq(&actual, &expect) {
            let mut inputs = vector.clone();
            inputs.as_object_mut().unwrap().shift_remove("expect");
            failures.push(format!(
                "{label}[{i}] {}\n  expected: {}\n  actual:   {}",
                inputs.to_json().unwrap(),
                expect.to_json().unwrap(),
                actual.to_json().unwrap()
            ));
        }
    }
    failures
}
