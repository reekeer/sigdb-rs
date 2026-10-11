//! Rules to `.sigdb`: per-index JSON payloads, per-group automata, data sections, container.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::database::{io_error, read_file};
use crate::error::{Error, Result};
use crate::format::automaton::{build_automaton, serialize_automaton};
use crate::format::container::{dump_json, write_container};
use crate::groups::{
    GroupKind, MatchMode, RESERVED_RULE_KEYS, check_name, normalize, parse_values, resolve_groups,
};
use crate::hash::FxHashMap;
use crate::json::{self, Writer};
use crate::text::{display_path, repr_str};
use crate::types::{BuildResult, Section};
use crate::value::{Map, Value};

pub const DEFAULT_INDEX: &str = "main";
pub const FORMAT_NAME: &str = "SIGDB";

pub fn index_section(index: &str) -> String {
    format!("index/{index}")
}

pub fn automaton_section(index: &str, group: &str) -> String {
    format!("automaton/{index}/{group}")
}

pub fn json_section(name: &str) -> String {
    format!("json/{name}")
}

pub fn blob_section(name: &str) -> String {
    format!("blob/{name}")
}

/// Options of [`build`] and [`build_bytes`], the keyword arguments of the Python functions.
#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Group configuration of the `main` index: `{name: {kind, match, ignore_case, trim}}`.
    pub groups: Option<Value>,
    /// More indexes: `{name: {"rules": ..., "groups": ...}}`.
    pub indexes: Option<Value>,
    /// Named data sections: JSON values or bytes.
    pub sections: IndexMap<String, Section>,
    /// Header fields; `format` defaults to `SIGDB`.
    pub metadata: Option<Value>,
    /// Adds `created` and `build` (UTC date) to the header. Builds without it are reproducible.
    pub timestamp: Option<i64>,
    pub zstd_level: i32,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            groups: None,
            indexes: None,
            sections: IndexMap::new(),
            metadata: None,
            timestamp: None,
            zstd_level: 19,
        }
    }
}

impl BuildOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the whole group configuration of the `main` index.
    pub fn groups(mut self, groups: impl Into<Value>) -> Self {
        self.groups = Some(groups.into());
        self
    }

    /// Configures one group of the `main` index.
    pub fn group(mut self, name: &str, config: impl Into<Value>) -> Self {
        let groups = self.groups.get_or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(m) = groups {
            m.insert(name.to_string(), config.into());
        }
        self
    }

    /// Adds an index with its rules and group configuration.
    pub fn index(mut self, name: &str, rules: impl Into<Value>, groups: Option<Value>) -> Self {
        let indexes = self.indexes.get_or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(m) = indexes {
            let mut spec = Map::new();
            spec.insert("rules".into(), rules.into());
            if let Some(g) = groups {
                spec.insert("groups".into(), g);
            }
            m.insert(name.to_string(), Value::Object(spec));
        }
        self
    }

    pub fn section(mut self, name: &str, value: impl Into<Section>) -> Self {
        self.sections.insert(name.to_string(), value.into());
        self
    }

    pub fn metadata(mut self, metadata: impl Into<Value>) -> Self {
        self.metadata = Some(metadata.into());
        self
    }

    pub fn timestamp(mut self, timestamp: i64) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    pub fn zstd_level(mut self, level: i32) -> Self {
        self.zstd_level = level;
        self
    }
}

/// Merges a rule set (an object) or a list of rule sets into one ordered map.
pub fn merge_rules(rules: &Value) -> Result<IndexMap<&str, &Map>> {
    let parts: Vec<&Value> = match rules {
        Value::Object(_) => vec![rules],
        Value::Array(list) => list.iter().collect(),
        _ => return Err(Error::format("rules must be a JSON object")),
    };
    let mut merged: IndexMap<&str, &Map> = IndexMap::new();
    for part in parts {
        let Value::Object(part) = part else {
            return Err(Error::format("rules must be a JSON object"));
        };
        for (key, value) in part {
            if key.is_empty() {
                return Err(Error::format("rule keys must be non-empty strings"));
            }
            if merged.contains_key(key.as_str()) {
                return Err(Error::format(format!("duplicate rule key: {}", repr_str(key))));
            }
            let Value::Object(rule) = value else {
                return Err(Error::format(format!("rule {} must be an object", repr_str(key))));
            };
            merged.insert(key, rule);
        }
    }
    Ok(merged)
}

/// Compiles one index into its sections: `index/<name>` and one `automaton/<name>/<group>` per
/// non-exact group that has patterns.
pub fn compile_index(name: &str, rules: &Value, groups: Option<&Value>) -> Result<Vec<(String, Vec<u8>)>> {
    let specs = resolve_groups(groups)?;
    let merged = merge_rules(rules)?;

    let mut pattern_ids: Vec<FxHashMap<String, u32>> = vec![FxHashMap::default(); specs.len()];
    let mut patterns: Vec<(u32, String)> = Vec::new();
    let mut pattern_items: Vec<Vec<u32>> = Vec::new();

    let mut out = Writer::new();
    out.raw("{\"groups\":{");
    for (i, (g, spec)) in specs.iter().enumerate() {
        if i > 0 {
            out.raw(",");
        }
        out.string(g);
        out.raw(":");
        out.value(&spec.to_json()).map_err(|e| Error::format(e.0))?;
    }
    out.raw("},\"items\":[");

    for (item_id, (key, rule)) in merged.iter().enumerate() {
        let item_id = item_id as u32;
        let data = rule.get("data").unwrap_or(&Value::Null);
        if json::dump(data).is_err() {
            return Err(Error::format(format!("rule {}: data is not serializable as JSON", repr_str(key))));
        }
        for (group, raw) in rule.iter() {
            if RESERVED_RULE_KEYS.contains(&group.as_str()) {
                continue;
            }
            let Some(gi) = specs.get_index_of(group) else {
                return Err(Error::format(format!("rule {}: unknown group: {group}", repr_str(key))));
            };
            let spec = &specs[gi];
            for value in parse_values(spec, Some(raw))? {
                let text = normalize(spec, &value);
                if text.is_empty() {
                    return Err(Error::format(format!(
                        "rule {}: empty pattern in group {group}",
                        repr_str(key)
                    )));
                }
                if spec.kind == GroupKind::Map && text.starts_with(':') {
                    return Err(Error::format(format!(
                        "rule {}: empty name in group {group}",
                        repr_str(key)
                    )));
                }
                let pid = match pattern_ids[gi].get(&text) {
                    Some(&pid) => pid,
                    None => {
                        let pid = patterns.len() as u32;
                        pattern_ids[gi].insert(text.clone(), pid);
                        patterns.push((gi as u32, text));
                        pattern_items.push(Vec::new());
                        pid
                    }
                };
                let ids = &mut pattern_items[pid as usize];
                if ids.last() != Some(&item_id) {
                    ids.push(item_id);
                }
            }
        }
        if item_id > 0 {
            out.raw(",");
        }
        out.raw("[");
        out.string(key);
        out.raw(",");
        out.value(data).map_err(|e| Error::format(e.0))?;
        out.raw("]");
    }

    out.raw("],\"patterns\":[");
    for (pid, (gi, text)) in patterns.iter().enumerate() {
        if pid > 0 {
            out.raw(",");
        }
        out.raw("[");
        out.string(specs.get_index(*gi as usize).map_or("", |(g, _)| g.as_str()));
        out.raw(",");
        out.string(text);
        out.raw("]");
    }
    out.raw("],\"pattern_items\":[");
    for (pid, ids) in pattern_items.iter().enumerate() {
        if pid > 0 {
            out.raw(",");
        }
        out.raw("[");
        for (k, id) in ids.iter().enumerate() {
            if k > 0 {
                out.raw(",");
            }
            out.uint(*id as usize);
        }
        out.raw("]");
    }
    out.raw("]}");

    let mut sections = vec![(index_section(name), out.out.into_bytes())];
    for (gi, (group, spec)) in specs.iter().enumerate() {
        if spec.match_mode == MatchMode::Exact {
            continue;
        }
        let entries: Vec<(&[u8], u32)> = patterns
            .iter()
            .enumerate()
            .filter(|(_, (g, _))| *g as usize == gi)
            .map(|(pid, (_, text))| (text.as_bytes(), pid as u32))
            .collect();
        if !entries.is_empty() {
            let automaton = serialize_automaton(&build_automaton(&entries));
            sections.push((automaton_section(name, group), automaton));
        }
    }
    Ok(sections)
}

/// `YYYY-MM-DD` (UTC) of a Unix timestamp.
fn utc_date(timestamp: i64) -> String {
    let days = timestamp.div_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn make_header(metadata: Option<&Value>, timestamp: Option<i64>) -> Result<Map> {
    let mut header = match metadata {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(Error::format("metadata must be an object")),
    };
    header.entry("format".into()).or_insert_with(|| FORMAT_NAME.into());
    if let Some(ts) = timestamp {
        if ts < 0 {
            return Err(Error::format("timestamp must be a non-negative integer"));
        }
        header.entry("created".into()).or_insert(Value::Int(ts));
        header.entry("build".into()).or_insert_with(|| utc_date(ts).into());
    }
    Ok(header)
}

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::BigInt(_) => true,
        Value::Float(f) => *f != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

fn collect_sections(rules: Option<&Value>, options: &BuildOptions) -> Result<Vec<(String, Vec<u8>)>> {
    let mut specs: IndexMap<String, (&Value, Option<&Value>)> = IndexMap::new();
    let groups = options.groups.as_ref().filter(|g| !g.is_null());
    match rules.filter(|r| !r.is_null()) {
        Some(rules) => {
            specs.insert(DEFAULT_INDEX.to_string(), (rules, groups.filter(|g| truthy(g))));
        }
        None if groups.is_some() => return Err(Error::format("groups requires rules")),
        None => {}
    }
    match options.indexes.as_ref().filter(|i| truthy(i)) {
        None => {}
        Some(Value::Object(indexes)) => {
            for (name, spec) in indexes {
                check_name(name, "index")?;
                if specs.contains_key(name) {
                    return Err(Error::format(format!("duplicate index: {name}")));
                }
                let rules = spec.get("rules").filter(|r| !r.is_null());
                let (Value::Object(_), Some(rules)) = (spec, rules) else {
                    return Err(Error::format(format!("index {name} must have rules")));
                };
                specs.insert(name.clone(), (rules, spec.get("groups")));
            }
        }
        Some(_) => return Err(Error::format("indexes must be an object")),
    }
    if specs.is_empty() {
        return Err(Error::format("at least one index is required"));
    }

    let mut out = Vec::new();
    for (name, (rules, groups)) in &specs {
        out.extend(compile_index(name, rules, *groups)?);
    }
    for (name, value) in &options.sections {
        check_name(name, "section")?;
        match value {
            Section::Blob(b) => out.push((blob_section(name), b.clone())),
            Section::Json(v) => out.push((json_section(name), dump_json(v)?)),
        }
    }
    Ok(out)
}

fn build_internal(
    rules: Option<&Value>,
    options: &BuildOptions,
) -> Result<(Vec<u8>, Map, IndexMap<String, String>)> {
    let header = make_header(options.metadata.as_ref(), options.timestamp)?;
    let sections = collect_sections(rules, options)?;
    let (data, hashes) = write_container(&header, &sections, options.zstd_level)?;
    Ok((data, header, hashes))
}

/// Compiles rules into `.sigdb` bytes.
pub fn build_bytes(rules: Option<&Value>, options: &BuildOptions) -> Result<Vec<u8>> {
    Ok(build_internal(rules, options)?.0)
}

/// Compiles rules into a `.sigdb` file, creating its directory.
pub fn build(
    rules: Option<&Value>,
    output_path: impl AsRef<Path>,
    options: &BuildOptions,
) -> Result<BuildResult> {
    let (data, metadata, sections) = build_internal(rules, options)?;
    let output = output_path.as_ref();
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| io_error(&e, parent))?;
    }
    std::fs::write(output, &data).map_err(|e| io_error(&e, output))?;
    Ok(BuildResult { output_path: output.to_path_buf(), size: data.len(), metadata, sections })
}

fn read_json_rules(path: &Path) -> Result<Value> {
    let bytes = read_file(path)?;
    json::parse_bytes(&bytes)
        .map_err(|_| Error::format(format!("invalid rules json: {}", display_path(&path.to_string_lossy()))))
}

fn list_json_files(root: &Path, rel: &str, out: &mut Vec<String>) -> Result<()> {
    let dir = if rel.is_empty() { root.to_path_buf() } else { root.join(rel) };
    let entries = std::fs::read_dir(&dir).map_err(|e| io_error(&e, &dir))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_error(&e, &dir))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel_path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let full = root.join(&rel_path);
        let is_symlink = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        match std::fs::metadata(&full) {
            Ok(meta) if meta.is_dir() => {
                if !is_symlink {
                    list_json_files(root, &rel_path, out)?;
                }
            }
            Ok(meta) if meta.is_file() && name.ends_with(".json") => out.push(rel_path),
            _ => {}
        }
    }
    Ok(())
}

/// Reads rules from a JSON file, or from every `*.json` under a directory (sorted by relative
/// path, duplicate keys rejected). Keys keep the order of the files.
pub fn read_rules(path: impl AsRef<Path>) -> Result<Map> {
    let p = path.as_ref();
    let to_map = |merged: IndexMap<&str, &Map>| -> Map {
        merged.into_iter().map(|(k, v)| (k.to_string(), Value::Object(v.clone()))).collect()
    };
    if p.is_file() {
        let value = read_json_rules(p)?;
        return Ok(to_map(merge_rules(&value)?));
    }
    if !p.is_dir() {
        return Err(Error::format(format!("rules path not found: {}", display_path(&p.to_string_lossy()))));
    }
    let mut files = Vec::new();
    list_json_files(p, "", &mut files)?;
    files.sort();
    let mut merged = Map::new();
    let mut origin: HashMap<String, String> = HashMap::new();
    for rel in files {
        let full: PathBuf = p.join(&rel);
        let value = read_json_rules(&full)?;
        for (key, rule) in merge_rules(&value)? {
            if let Some(seen) = origin.get(key) {
                return Err(Error::format(format!(
                    "duplicate rule key {} in {seen} and {rel}",
                    repr_str(key)
                )));
            }
            merged.insert(key.to_string(), Value::Object(rule.clone()));
            origin.insert(key.to_string(), rel.clone());
        }
    }
    Ok(merged)
}

/// Compiles a rules JSON file.
pub fn compile_json(
    json_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    options: &BuildOptions,
) -> Result<BuildResult> {
    let p = json_path.as_ref();
    if !p.is_file() {
        return Err(Error::format(format!("rules file not found: {}", display_path(&p.to_string_lossy()))));
    }
    let rules = Value::Object(read_rules(p)?);
    build(Some(&rules), output_path, options)
}

/// Compiles every `*.json` under a directory.
pub fn compile_dir(
    dir_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    options: &BuildOptions,
) -> Result<BuildResult> {
    let p = dir_path.as_ref();
    if !p.is_dir() {
        return Err(Error::format(format!(
            "rules directory not found: {}",
            display_path(&p.to_string_lossy())
        )));
    }
    let rules = Value::Object(read_rules(p)?);
    build(Some(&rules), output_path, options)
}

#[cfg(test)]
mod tests {
    use super::utc_date;

    #[test]
    fn dates() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_784_237_882), "2026-07-16");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
        assert_eq!(utc_date(253_402_300_799), "9999-12-31");
    }
}
