use std::path::{Path, PathBuf};

use sigdb::compiler::compile_index;
use sigdb::format::automaton::deserialize_automaton;
use sigdb::format::container::{dump_json, write_container};
use sigdb::{
    BuildOptions, Database, Error, LoadOptions, Map, Reader, Section, Value, build, build_bytes, compile_dir,
    compile_json, load, load_bytes, load_with, read_metadata, read_rules, validate,
};

fn json(s: &str) -> Value {
    Value::from_json(s).unwrap()
}

fn rules() -> Value {
    json(
        r#"{
        "react@18/index.js": {"data": {"package": "react", "entry": true}, "features": ["P:useState", "K:setState"]},
        "zustand@4/index.js": {"data": {"package": "zustand"}, "features": ["P:useState"]},
        "nginx": {"headers": {"Server": "nginx"}}
    }"#,
    )
}

fn groups() -> BuildOptions {
    BuildOptions::new().group("features", json("{}"))
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("sigdb-rs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn format_err<T: std::fmt::Debug>(r: Result<T, Error>, text: &str) {
    match r {
        Err(Error::Format(m)) => assert!(m.contains(text), "{m:?} does not contain {text:?}"),
        other => panic!("expected FormatError containing {text:?}, got {other:?}"),
    }
}

#[test]
fn build_load_read() {
    let tmp = TempDir::new("api");
    let out = tmp.path().join("a.sigdb");
    let opts = groups().metadata(json(r#"{"dataset": "x"}"#));
    let result = build(Some(&rules()), &out, &opts).unwrap();
    assert_eq!(result.size as u64, std::fs::metadata(&out).unwrap().len());
    assert_eq!(Value::Object(result.metadata.clone()), json(r#"{"dataset": "x", "format": "SIGDB"}"#));
    assert_eq!(read_metadata(&out).unwrap(), result.metadata);
    let mut a: Vec<_> = result.sections.keys().cloned().collect();
    let mut b: Vec<_> = load(&out).unwrap().section_digests().keys().cloned().collect();
    a.sort();
    b.sort();
    assert_eq!(a, b);

    let first = std::fs::read(&out).unwrap();
    build(Some(&rules()), &out, &opts).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), first, "rebuild is byte-identical");

    let stamped = build_bytes(Some(&rules()), &groups().timestamp(86_400)).unwrap();
    let meta = load_bytes(stamped).unwrap().metadata;
    assert_eq!(
        (meta["created"].clone(), meta["build"].clone()),
        (Value::Int(86_400), Value::from("1970-01-02"))
    );
    format_err(
        build_bytes(Some(&rules()), &groups().timestamp(-1)),
        "timestamp must be a non-negative integer",
    );

    let v = validate(&out);
    assert!(v.ok, "{:?}", v.errors);
    assert_eq!(v.sections.as_ref(), Some(&result.sections));

    let reader = Reader::new(&out);
    let tokens = reader.match_tokens("features", &["P:useState"]).unwrap();
    assert_eq!(tokens.into_iter().collect::<Vec<_>>(), vec![(0, 1), (1, 1)]);
    assert!(std::ptr::eq(reader.database().unwrap(), reader.database().unwrap()));
    assert_eq!(reader.item("nginx").unwrap(), Some(&reader.main().unwrap().items[2]));
    assert_eq!(reader.path(), out.as_path());
    assert_eq!(reader.metadata().unwrap(), result.metadata);
    assert!(reader.validate().ok);

    let eager = load_with(&out, &LoadOptions { lazy: false, ..Default::default() }).unwrap();
    assert_eq!(eager.r#match("Server: nginx").unwrap().item_id, Some(2));

    let db = load(&out).unwrap();
    let clone = Database::from_raw(db.metadata.clone(), db.raw_sections().unwrap()).unwrap();
    let t = clone.match_tokens("features", &["K:setState"]).unwrap();
    assert_eq!(t.into_iter().collect::<Vec<_>>(), vec![(0, 1)]);
    assert_eq!(clone.section_digests(), db.section_digests());
    assert_eq!(
        db.item("react@18/index.js").unwrap().unwrap().data,
        json(r#"{"package": "react", "entry": true}"#)
    );
}

#[test]
fn rules_directories() {
    let tmp = TempDir::new("rules");
    let dir = tmp.path().join("rules");
    std::fs::create_dir_all(dir.join("b")).unwrap();
    std::fs::write(dir.join("a.json"), r#"{"x": {"js": "x"}}"#).unwrap();
    std::fs::write(dir.join("b/c.json"), r#"{"y": {"js": "y"}}"#).unwrap();
    std::fs::write(dir.join("b/ignored.txt"), "{").unwrap();
    assert_eq!(read_rules(&dir).unwrap().keys().collect::<Vec<_>>(), ["x", "y"]);
    let compiled =
        compile_dir(&dir, tmp.path().join("dir.sigdb"), &BuildOptions::new().timestamp(0)).unwrap();
    assert_eq!(compiled.metadata["build"], Value::from("1970-01-01"));
    let keys: Vec<String> =
        load(tmp.path().join("dir.sigdb")).unwrap().items().unwrap().iter().map(|i| i.key.clone()).collect();
    assert_eq!(keys, ["x", "y"]);
    compile_json(dir.join("a.json"), tmp.path().join("file.sigdb"), &BuildOptions::new()).unwrap();
    format_err(compile_json(&dir, tmp.path().join("x.sigdb"), &BuildOptions::new()), "not found");
    format_err(
        compile_dir(dir.join("a.json"), tmp.path().join("x.sigdb"), &BuildOptions::new()),
        "not found",
    );
    format_err(read_rules(tmp.path().join("missing")), "rules path not found");

    std::fs::write(dir.join("b/dup.json"), r#"{"x": {}}"#).unwrap();
    format_err(read_rules(&dir), "duplicate rule key 'x' in a.json and b/dup.json");
    std::fs::write(dir.join("b/dup.json"), "{").unwrap();
    format_err(read_rules(&dir), "invalid rules json");
}

#[test]
fn rule_files_sort_by_code_point() {
    let tmp = TempDir::new("sort");
    std::fs::write(tmp.path().join("\u{ff21}.json"), r#"{"bmp": {}}"#).unwrap();
    std::fs::write(tmp.path().join("\u{1f600}.json"), r#"{"astral": {}}"#).unwrap();
    std::fs::write(tmp.path().join("Z.json"), r#"{"ascii": {}}"#).unwrap();
    assert_eq!(read_rules(tmp.path()).unwrap().keys().collect::<Vec<_>>(), ["ascii", "bmp", "astral"]);
}

#[test]
fn unserializable_values() {
    let mut rule = Map::new();
    rule.insert("data".into(), Value::Float(f64::NAN));
    let mut rules = Map::new();
    rules.insert("a".into(), Value::Object(rule));
    format_err(
        build_bytes(Some(&Value::Object(rules)), &BuildOptions::new()),
        "rule 'a': data is not serializable as JSON",
    );
    format_err(
        build_bytes(Some(&json("{}")), &BuildOptions::new().section("s", Value::Float(f64::INFINITY))),
        "value is not serializable as JSON: Out of range float values are not JSON compliant: inf",
    );
    format_err(
        build_bytes(Some(&json("{}")), &BuildOptions::new().metadata(json("[]"))),
        "metadata must be an object",
    );
}

#[test]
fn automaton_and_payload_validation() {
    let sections: Map = compile_index("main", &json(r#"{"a": {"js": "abc"}}"#), None)
        .unwrap()
        .into_iter()
        .map(|(n, b)| (n, Value::String(String::from_utf8_lossy(&b).into_owned())))
        .collect();
    let raw = compile_index("main", &json(r#"{"a": {"js": "abc"}}"#), None).unwrap();
    let automaton = raw.iter().find(|(n, _)| n == "automaton/main/js").unwrap().1.clone();
    let index = raw.iter().find(|(n, _)| n == "index/main").unwrap().1.clone();
    assert!(sections.contains_key("index/main"));
    format_err(deserialize_automaton(&automaton, 0), "unknown pattern");
    format_err(deserialize_automaton(&[0xff, 0xff, 0xff, 0x01, 0x00, 0x00], 1), "exceed");

    let payload = sigdb::json::parse_bytes(&index).unwrap();
    type Mutation = Box<dyn Fn(&mut Map)>;
    let mutations: Vec<(Mutation, &str)> = vec![
        (
            Box::new(|p| {
                p.insert("pattern_items".into(), json("[[5]]"));
            }),
            "unknown item",
        ),
        (
            Box::new(|p| {
                p.insert("pattern_items".into(), json("[[]]"));
            }),
            "non-empty array",
        ),
        (
            Box::new(|p| {
                p.insert("patterns".into(), json(r#"[["nope", "x"]]"#));
            }),
            "unknown group",
        ),
        (
            Box::new(|p| {
                p.insert("items".into(), json(r#"[["a"]]"#));
            }),
            "[key, data]",
        ),
        (
            Box::new(|p| {
                let g = p.get_mut("groups").unwrap().as_object_mut().unwrap();
                g.get_mut("js").unwrap().as_object_mut().unwrap().insert("match".into(), "regex".into());
            }),
            "match must be",
        ),
    ];
    for (mutate, text) in mutations {
        let mut bad = payload.as_object().unwrap().clone();
        mutate(&mut bad);
        let (data, _) = write_container(
            &Map::new(),
            &[("index/main".into(), dump_json(&Value::Object(bad)).unwrap())],
            1,
        )
        .unwrap();
        format_err(load_bytes(data).unwrap().index("main"), text);
    }

    let tmp = TempDir::new("stray");
    let (stray, _) = write_container(
        &Map::new(),
        &[("index/main".into(), index.clone()), ("automaton/other/js".into(), automaton.clone())],
        1,
    )
    .unwrap();
    std::fs::write(tmp.path().join("stray.sigdb"), stray).unwrap();
    let v = validate(tmp.path().join("stray.sigdb"));
    assert!(!v.ok && v.errors[0].contains("unknown index"), "{:?}", v.errors);

    let (unknown, _) = write_container(&Map::new(), &[("weird/x".into(), Vec::new())], 1).unwrap();
    format_err(load_bytes(unknown), "unknown section");

    format_err(
        deserialize_automaton(&[2, 1, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0x61, 1], 1),
        "fail links do not terminate",
    );
}

#[test]
fn index_api() {
    let mut data = Map::new();
    data.insert("10".into(), 1.into());
    data.insert("2".into(), 2.into());
    let rules = Value::from_iter([
        (
            "10",
            Value::from_iter([("features", json(r#"["a", "b"]"#)), ("data", Value::Object(data.clone()))]),
        ),
        ("2", Value::from_iter([("features", json(r#"["b"]"#)), ("strings", json(r#""needle""#))])),
    ]);
    let opts = BuildOptions::new()
        .groups(json(r#"{"features": {}, "strings": {"match": "contains"}}"#))
        .section("blob", vec![1u8, 2])
        .section("meta", json(r#"{"k": [1, 0.5]}"#));
    let db = load_bytes(build_bytes(Some(&rules), &opts).unwrap()).unwrap();
    let index = db.main().unwrap();
    assert_eq!(db.index_names(), ["main"]);
    assert_eq!(db.section_names(), ["blob", "meta"]);
    assert_eq!(db.section("blob").unwrap(), &Section::Blob(vec![1, 2]));
    assert_eq!(db.section("meta").unwrap(), &Section::Json(json(r#"{"k": [1, 0.5]}"#)));
    assert_eq!(db.items().unwrap().iter().map(|i| i.key.as_str()).collect::<Vec<_>>(), ["10", "2"]);
    assert_eq!(index.items[0].data, Value::Object(data));
    assert_eq!(index.pattern_total(), 3);
    let p = index.pattern(1).unwrap();
    assert_eq!((p.id, p.group.as_str(), p.text.as_str(), p.item_ids), (1, "features", "b", vec![0, 1]));
    assert_eq!(index.item_id("2"), Some(1));
    assert_eq!(index.pattern_count(0, None), Some(2));
    assert_eq!(index.pattern_count(1, Some("strings")), Some(1));
    assert_eq!(index.pattern_count(9, None), None);
    assert_eq!(index.spec("strings").unwrap().match_mode, sigdb::MatchMode::Contains);
    format_err(index.spec("nope"), "unknown group: nope");
    let hits = index.hits_for([1, 0, 1]).unwrap();
    assert_eq!(hits.iter().map(|h| (h.item_id, h.hits)).collect::<Vec<_>>(), vec![(0, 2), (1, 1)]);
    assert_eq!(index.hits_for([9]).unwrap_err(), Error::Format("unknown pattern id: 9".into()));
    assert!(index.scan("xx NEEDLE", "strings").unwrap().is_empty());
    let occ = index.scan("xx needle", "strings").unwrap();
    assert_eq!((occ[0].pattern_id, occ[0].start, occ[0].end), (2, 3, 9));
    let all = db.match_all("features", &["a", "b"]).unwrap();
    assert_eq!(all.iter().map(|h| h.item_id).collect::<Vec<_>>(), vec![0, 1]);
    format_err(db.match_tokens("features", "a"), "tokens must be a list of strings");
    format_err(db.match_all("features", &json("5")), "features values must be a string, list, or object");
    index.preload().unwrap();
}

#[test]
fn readme_example() {
    let rules = json(
        r#"{
        "nginx": {"headers": {"Server": "nginx"}},
        "react@18.2.0/index.js": {
            "data": {"package": "react", "version": "18.2.0", "entry": true},
            "features": ["P:useState", "S:react.element"],
            "strings": ["Minified React error #"]
        },
        "zustand@4.5.0/index.js": {
            "data": {"package": "zustand", "version": "4.5.0"},
            "features": ["P:useState", "K:subscribe"]
        }
    }"#,
    );
    let tmp = TempDir::new("readme");
    let path = tmp.path().join("libs.sigdb");
    let opts = BuildOptions::new()
        .groups(json(r#"{"features": {}, "strings": {"match": "contains"}}"#))
        .section("prints", json(r#"{"version": 1}"#));
    build(Some(&rules), &path, &opts).unwrap();
    let db = load(&path).unwrap();
    assert_eq!(db.r#match("Server: nginx/1.25.3").unwrap().key(), Some("nginx"));
    let t = db.match_tokens("features", &["P:useState", "K:subscribe"]).unwrap();
    assert_eq!(t.into_iter().collect::<Vec<_>>(), vec![(2, 2), (1, 1)]);
    let keys: Vec<&str> =
        db.match_all("features", &["P:useState"]).unwrap().iter().map(|h| h.item.key.as_str()).collect();
    assert_eq!(keys, ["react@18.2.0/index.js", "zustand@4.5.0/index.js"]);
    let occ = db.scan("throw Error('Minified React error #321')", "strings").unwrap();
    assert_eq!((occ[0].pattern_id, occ[0].start, occ[0].end), (3, 13, 35));
    assert_eq!(db.item("react@18.2.0/index.js").unwrap().unwrap().data["version"], Value::from("18.2.0"));
    assert_eq!(db.items_with_prefix("react@").unwrap().len(), 1);
    assert_eq!(db.section("prints").unwrap().as_json().unwrap(), &json(r#"{"version": 1}"#));
}
