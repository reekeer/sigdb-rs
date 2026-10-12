use indexmap::IndexMap;

use crate::error::{Error, Result};
use crate::text::{lower, repr_list, repr_str, strip};
use crate::value::{Map, Value};

pub const BUILTIN_GROUPS: [&str; 19] = [
    "headers",
    "js",
    "meta",
    "html",
    "script_src",
    "css",
    "url",
    "path",
    "file",
    "dns",
    "subdomain",
    "link",
    "json",
    "api",
    "tls",
    "server",
    "framework",
    "cms",
    "cdn",
];

pub const BUILTIN_MAP_GROUPS: [&str; 2] = ["headers", "meta"];
pub const RESERVED_RULE_KEYS: [&str; 1] = ["data"];

const KINDS: [&str; 2] = ["list", "map"];
const MODES: [&str; 3] = ["prefix", "contains", "exact"];
const CONFIG_KEYS: [&str; 4] = ["kind", "match", "ignore_case", "trim"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupKind {
    List,
    Map,
}

impl GroupKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GroupKind::List => "list",
            GroupKind::Map => "map",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchMode {
    Prefix,
    Contains,
    Exact,
}

impl MatchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchMode::Prefix => "prefix",
            MatchMode::Contains => "contains",
            MatchMode::Exact => "exact",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupSpec {
    pub name: String,
    pub kind: GroupKind,
    pub match_mode: MatchMode,
    pub ignore_case: bool,
    pub trim: bool,
}

impl GroupSpec {
    pub fn to_json(&self) -> Value {
        let mut m = Map::with_capacity(4);
        m.insert("kind".into(), self.kind.as_str().into());
        m.insert("match".into(), self.match_mode.as_str().into());
        m.insert("ignore_case".into(), self.ignore_case.into());
        m.insert("trim".into(), self.trim.into());
        Value::Object(m)
    }
}

pub fn check_name<'a>(name: &'a str, what: &str) -> Result<&'a str> {
    let b = name.as_bytes();
    let first = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let rest = |c: u8| first(c) || c == b'.' || c == b'-';
    let ok = !b.is_empty() && b.len() <= 128 && first(b[0]) && b[1..].iter().all(|&c| rest(c));
    if !ok {
        return Err(Error::format(format!("invalid {what} name: {}", repr_str(name))));
    }
    Ok(name)
}

pub fn builtin_spec(name: &str) -> GroupSpec {
    GroupSpec {
        name: name.to_string(),
        kind: if BUILTIN_MAP_GROUPS.contains(&name) { GroupKind::Map } else { GroupKind::List },
        match_mode: if name == "headers" { MatchMode::Contains } else { MatchMode::Prefix },
        ignore_case: true,
        trim: true,
    }
}

pub fn resolve_groups(config: Option<&Value>) -> Result<IndexMap<String, GroupSpec>> {
    let mut specs: IndexMap<String, GroupSpec> =
        BUILTIN_GROUPS.iter().map(|&n| (n.to_string(), builtin_spec(n))).collect();
    let Some(config) = config.filter(|c| !c.is_null()) else {
        return Ok(specs);
    };
    let Value::Object(config) = config else {
        return Err(Error::format("groups must be an object"));
    };
    for (name, cfg) in config {
        check_name(name, "group")?;
        if RESERVED_RULE_KEYS.contains(&name.as_str()) {
            return Err(Error::format(format!("group name is reserved: {name}")));
        }
        let Value::Object(cfg) = cfg else {
            return Err(Error::format(format!("group {name} config must be an object")));
        };
        let mut unknown: Vec<&str> =
            cfg.keys().map(String::as_str).filter(|k| !CONFIG_KEYS.contains(k)).collect();
        if !unknown.is_empty() {
            unknown.sort_unstable();
            unknown.dedup();
            return Err(Error::format(format!(
                "group {name} has unknown config keys: {}",
                repr_list(&unknown)
            )));
        }
        let base = specs.get(name).cloned().unwrap_or_else(|| GroupSpec {
            name: name.clone(),
            kind: GroupKind::List,
            match_mode: MatchMode::Exact,
            ignore_case: false,
            trim: false,
        });
        let mut merged = match base.to_json() {
            Value::Object(m) => m,
            _ => unreachable!(),
        };
        for (k, v) in cfg {
            merged.insert(k.clone(), v.clone());
        }
        let spec = spec_from_json(name, &Value::Object(merged))?;
        specs.insert(name.clone(), spec);
    }
    Ok(specs)
}

pub fn spec_from_json(name: &str, raw: &Value) -> Result<GroupSpec> {
    let Value::Object(cfg) = raw else {
        return Err(Error::format(format!("group {name} config must be an object")));
    };
    let kind = match cfg.get("kind").and_then(Value::as_str) {
        Some("list") => GroupKind::List,
        Some("map") => GroupKind::Map,
        _ => {
            return Err(Error::format(format!("group {name}: kind must be one of {}", repr_list(&KINDS))));
        }
    };
    let match_mode = match cfg.get("match").and_then(Value::as_str) {
        Some("prefix") => MatchMode::Prefix,
        Some("contains") => MatchMode::Contains,
        Some("exact") => MatchMode::Exact,
        _ => {
            return Err(Error::format(format!("group {name}: match must be one of {}", repr_list(&MODES))));
        }
    };
    let (Some(ignore_case), Some(trim)) =
        (cfg.get("ignore_case").and_then(Value::as_bool), cfg.get("trim").and_then(Value::as_bool))
    else {
        return Err(Error::format(format!("group {name}: ignore_case and trim must be booleans")));
    };
    Ok(GroupSpec { name: name.to_string(), kind, match_mode, ignore_case, trim })
}

pub fn normalize(spec: &GroupSpec, value: &str) -> String {
    let trimmed;
    let mut value = value;
    if spec.trim {
        if spec.kind == GroupKind::Map {
            match value.find(':') {
                None => value = strip(value),
                Some(i) => {
                    trimmed = format!("{}:{}", strip(&value[..i]), strip(&value[i + 1..]));
                    value = &trimmed;
                }
            }
        } else {
            value = strip(value);
        }
    }
    if spec.ignore_case { lower(value) } else { value.to_string() }
}

pub fn format_map_value(name: &str, value: &str) -> String {
    format!("{name}:{value}")
}

pub fn parse_string_map(value: Option<&Value>, group: &str) -> Result<Vec<(String, String)>> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| match v {
                Value::String(s) => Ok((k.clone(), s.clone())),
                _ => Err(Error::format(format!("{group} keys/values must be strings"))),
            })
            .collect(),
        Some(_) => Err(Error::format(format!("{group} must be an object"))),
    }
}

pub fn parse_string_list(value: Option<&Value>, group: &str) -> Result<Vec<String>> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => Ok(s.clone()),
                _ => Err(Error::format(format!("{group} items must be strings"))),
            })
            .collect(),
        Some(_) => Err(Error::format(format!("{group} must be a string or list of strings"))),
    }
}

pub fn parse_html_list(value: Option<&Value>) -> Result<Vec<String>> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        Some(Value::Object(m)) => Ok(vec![html_spec_to_value(m)?]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => Ok(s.clone()),
                Value::Object(m) => html_spec_to_value(m),
                _ => Err(Error::format("html items must be strings or objects")),
            })
            .collect(),
        Some(_) => Err(Error::format("html must be a string, object, or list")),
    }
}

pub fn parse_values(spec: &GroupSpec, value: Option<&Value>) -> Result<Vec<String>> {
    if spec.kind == GroupKind::Map {
        let pairs = parse_string_map(value, &spec.name)?;
        return Ok(pairs.iter().map(|(k, v)| format_map_value(k, v)).collect());
    }
    if spec.name == "html" {
        return parse_html_list(value);
    }
    parse_string_list(value, &spec.name)
}

fn html_spec_to_value(spec: &Map) -> Result<String> {
    if spec.keys().any(|k| !matches!(k.as_str(), "tag" | "attr" | "value")) {
        return Err(Error::format("html spec has invalid keys"));
    }
    let get = |k: &str| spec.get(k).filter(|v| !v.is_null());
    let (tag, attr, value) = (get("tag"), get("attr"), get("value"));
    if let Some(t) = tag {
        if t.as_str().is_none_or(str::is_empty) {
            return Err(Error::format("html tag must be a non-empty string"));
        }
    }
    if let Some(a) = attr {
        if a.as_str().is_none_or(str::is_empty) {
            return Err(Error::format("html attr must be a non-empty string"));
        }
    }
    if let Some(v) = value {
        if v.as_str().is_none() {
            return Err(Error::format("html value must be a string"));
        }
    }
    if value.is_some() && attr.is_none() {
        return Err(Error::format("html value requires attr"));
    }
    if tag.is_none() && attr.is_none() && value.is_none() {
        return Err(Error::format("html spec must include tag or attr"));
    }
    let mut parts: Vec<&str> = Vec::new();
    if let Some(t) = tag.and_then(Value::as_str) {
        parts.extend(["tag", t]);
    }
    if let Some(a) = attr.and_then(Value::as_str) {
        parts.extend(["attr", a]);
    }
    if let Some(v) = value.and_then(Value::as_str) {
        parts.extend(["value", v]);
    }
    Ok(parts.join(":"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_values() {
        assert_eq!(normalize(&builtin_spec("headers"), " Server :  NGINX "), "server:nginx");
        assert_eq!(normalize(&builtin_spec("meta"), "\u{3000}generator:WP\u{85}"), "generator:wp");
        assert_eq!(normalize(&builtin_spec("js"), "\u{1c}JQuery\u{1f}"), "jquery");
        assert_eq!(normalize(&builtin_spec("cms"), "\u{feff}Ghost"), "\u{feff}ghost");
        let cfg = Value::from_json(r#"{"f": {}, "m": {"kind": "map", "trim": true}}"#).unwrap();
        let specs = resolve_groups(Some(&cfg)).unwrap();
        assert_eq!(normalize(&specs["f"], " A:B "), " A:B ");
        assert_eq!(normalize(&specs["m"], " A : B "), "A:B");
    }

    #[test]
    fn names() {
        assert!(check_name("a.b-c_1", "group").is_ok());
        assert_eq!(check_name("bad/name", "group").unwrap_err().message(), "invalid group name: 'bad/name'");
        assert!(check_name(&"a".repeat(128), "x").is_ok());
        assert!(check_name(&"a".repeat(129), "x").is_err());
        assert!(check_name(".a", "x").is_err());
        assert!(check_name("a\n", "x").is_err());
    }
}
