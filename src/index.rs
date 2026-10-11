//! One index of a database: items, patterns and the per-group lookups built on first use.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use hashbrown::HashTable;

use indexmap::IndexMap;

use crate::error::{Error, Result};
use crate::format::automaton::Automaton;
use crate::groups::{
    GroupKind, GroupSpec, MatchMode, format_map_value, normalize, parse_values, spec_from_json,
};
use crate::hash::{FxHashMap, hash_str};
use crate::html::html_heads;
use crate::text::lower;
use crate::types::{Hit, Item, MatchResult, Occurrence, Pattern};
use crate::value::Value;

/// Loads the automaton of a group: `(group, pattern count) -> automaton`, `None` when the group
/// has no automaton section.
pub type AutomatonLoader = Box<dyn Fn(&str, usize) -> Result<Option<Automaton>> + Send + Sync>;

/// Values for `match_all` and `match_tokens`: one string, a list, `{name: value}` pairs for map
/// groups, or any JSON value with the Python package's rules.
#[derive(Debug, Clone)]
pub enum Values<'a> {
    One(&'a str),
    Many(Vec<&'a str>),
    Pairs(Vec<(&'a str, &'a str)>),
    Json(&'a Value),
}

impl<'a> From<&'a str> for Values<'a> {
    fn from(v: &'a str) -> Self {
        Values::One(v)
    }
}

impl<'a> From<&'a String> for Values<'a> {
    fn from(v: &'a String) -> Self {
        Values::One(v)
    }
}

impl<'a> From<&'a [&'a str]> for Values<'a> {
    fn from(v: &'a [&'a str]) -> Self {
        Values::Many(v.to_vec())
    }
}

impl<'a, const N: usize> From<&'a [&'a str; N]> for Values<'a> {
    fn from(v: &'a [&'a str; N]) -> Self {
        Values::Many(v.to_vec())
    }
}

impl<'a> From<Vec<&'a str>> for Values<'a> {
    fn from(v: Vec<&'a str>) -> Self {
        Values::Many(v)
    }
}

impl<'a> From<&'a [String]> for Values<'a> {
    fn from(v: &'a [String]) -> Self {
        Values::Many(v.iter().map(String::as_str).collect())
    }
}

impl<'a> From<&'a Vec<String>> for Values<'a> {
    fn from(v: &'a Vec<String>) -> Self {
        Values::Many(v.iter().map(String::as_str).collect())
    }
}

impl<'a> From<&'a [(&'a str, &'a str)]> for Values<'a> {
    fn from(v: &'a [(&'a str, &'a str)]) -> Self {
        Values::Pairs(v.to_vec())
    }
}

impl<'a> From<&'a Value> for Values<'a> {
    fn from(v: &'a Value) -> Self {
        Values::Json(v)
    }
}

/// An index: items, patterns, and lazily built exact-match tables and automata.
pub struct Index {
    pub name: String,
    pub groups: IndexMap<String, GroupSpec>,
    pub items: Vec<Item>,
    pattern_group: Vec<u32>,
    pattern_text: Vec<String>,
    item_offsets: Vec<u32>,
    item_ids: Vec<u32>,
    loader: AutomatonLoader,
    automata: Vec<OnceLock<Result<Option<Automaton>>>>,
    exact: Vec<OnceLock<HashTable<u32>>>,
    lengths: OnceLock<Vec<usize>>,
    key_ids: OnceLock<FxHashMap<String, usize>>,
    sorted_keys: OnceLock<Vec<usize>>,
    group_counts: Mutex<HashMap<Option<String>, Arc<Vec<usize>>>>,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Index")
            .field("name", &self.name)
            .field("groups", &self.groups.keys().collect::<Vec<_>>())
            .field("items", &self.items.len())
            .field("patterns", &self.pattern_text.len())
            .finish()
    }
}

fn err(name: &str, msg: &str) -> Error {
    Error::format(format!("index {name}: {msg}"))
}

/// The validated content of an index payload.
struct Parts {
    groups: IndexMap<String, GroupSpec>,
    items: Vec<Item>,
    pattern_group: Vec<u32>,
    pattern_text: Vec<String>,
    item_offsets: Vec<u32>,
    item_ids: Vec<u32>,
}

impl Parts {
    fn new(groups: IndexMap<String, GroupSpec>, items: Vec<Item>, patterns: usize) -> Parts {
        let mut item_offsets = Vec::with_capacity(patterns + 1);
        item_offsets.push(0);
        Parts {
            groups,
            items,
            pattern_group: Vec::with_capacity(patterns),
            pattern_text: Vec::with_capacity(patterns),
            item_offsets,
            item_ids: Vec::with_capacity(patterns),
        }
    }
}

/// Reads a payload in the exact shape the compiler writes — `{"groups":…,"items":…,"patterns":…,
/// "pattern_items":…}` with valid content — straight into the index structures. Anything else
/// returns `None` and goes through [`parse_payload`], which gives the same result for this shape
/// and the right error for any other.
fn parse_canonical(raw: &[u8]) -> Option<Parts> {
    use crate::json::Scanner;
    if raw.first() != Some(&b'{') {
        return None;
    }
    let text = std::str::from_utf8(raw).ok()?;
    let mut sc = Scanner::new(text);
    let key = |sc: &mut Scanner<'_>, name: &str| -> Option<()> {
        (sc.string()? == name && sc.eat(b':')).then_some(())
    };
    let list = |sc: &mut Scanner<'_>, each: &mut dyn FnMut(&mut Scanner<'_>) -> Option<()>| -> Option<()> {
        if !sc.eat(b'[') {
            return None;
        }
        if sc.eat(b']') {
            return Some(());
        }
        loop {
            each(sc)?;
            if sc.eat(b']') {
                return Some(());
            }
            if !sc.eat(b',') {
                return None;
            }
        }
    };

    if !sc.eat(b'{') {
        return None;
    }
    key(&mut sc, "groups")?;
    let Value::Object(groups_raw) = sc.value_at(1)? else { return None };
    let mut groups = IndexMap::with_capacity(groups_raw.len());
    for (g, cfg) in &groups_raw {
        groups.insert(g.clone(), spec_from_json(g, cfg).ok()?);
    }

    if !sc.eat(b',') {
        return None;
    }
    key(&mut sc, "items")?;
    let mut items = Vec::new();
    list(&mut sc, &mut |sc| {
        if !sc.eat(b'[') {
            return None;
        }
        let key = sc.string().filter(|k| !k.is_empty())?;
        if !sc.eat(b',') {
            return None;
        }
        let data = sc.value_at(3)?;
        if !sc.eat(b']') {
            return None;
        }
        items.push(Item { key, data });
        Some(())
    })?;

    let mut parts = Parts::new(groups, items, 0);
    if !sc.eat(b',') {
        return None;
    }
    key(&mut sc, "patterns")?;
    list(&mut sc, &mut |sc| {
        if !sc.eat(b'[') {
            return None;
        }
        let group = sc.string()?;
        if !sc.eat(b',') {
            return None;
        }
        let text = sc.string()?;
        if !sc.eat(b']') {
            return None;
        }
        let gi = parts.groups.get_index_of(&group)?;
        parts.pattern_group.push(gi as u32);
        parts.pattern_text.push(text);
        Some(())
    })?;

    if !sc.eat(b',') {
        return None;
    }
    key(&mut sc, "pattern_items")?;
    let item_count = parts.items.len() as u64;
    list(&mut sc, &mut |sc| {
        let start = parts.item_ids.len();
        list(sc, &mut |sc| {
            let id = sc.uint().filter(|&i| i < item_count)?;
            if parts.item_ids.len() > start && *parts.item_ids.last()? >= id as u32 {
                return None;
            }
            parts.item_ids.push(id as u32);
            Some(())
        })?;
        if parts.item_ids.len() == start {
            return None;
        }
        parts.item_offsets.push(parts.item_ids.len() as u32);
        Some(())
    })?;
    if parts.item_offsets.len() != parts.pattern_text.len() + 1 || !sc.eat(b'}') || !sc.end() {
        return None;
    }
    Some(parts)
}

/// Validates a parsed payload with the checks and messages of the Python package.
fn parse_payload(name: &str, payload: Value) -> Result<Parts> {
    let Value::Object(mut data) = payload else {
        return Err(err(name, "payload must be an object"));
    };

    let groups_raw = data.swap_remove("groups").unwrap_or(Value::Null);
    let Value::Object(groups_raw) = groups_raw else {
        return Err(err(name, "groups must be an object"));
    };
    let mut groups = IndexMap::with_capacity(groups_raw.len());
    for (g, cfg) in &groups_raw {
        groups.insert(g.clone(), spec_from_json(g, cfg)?);
    }

    let Some(Value::Array(items_raw)) = data.swap_remove("items") else {
        return Err(err(name, "items must be an array"));
    };
    let mut items = Vec::with_capacity(items_raw.len());
    for entry in items_raw {
        let Value::Array(pair) = entry else {
            return Err(err(name, "item must be [key, data]"));
        };
        if pair.len() != 2 {
            return Err(err(name, "item must be [key, data]"));
        }
        let mut it = pair.into_iter();
        let key = it.next().unwrap_or_default();
        let value = it.next().unwrap_or_default();
        match key {
            Value::String(k) if !k.is_empty() => items.push(Item { key: k, data: value }),
            _ => return Err(err(name, "item key must be a non-empty string")),
        }
    }

    let patterns_raw = data.swap_remove("patterns");
    let pattern_items_raw = data.swap_remove("pattern_items");
    let (Some(Value::Array(patterns_raw)), Some(Value::Array(pattern_items_raw))) =
        (patterns_raw, pattern_items_raw)
    else {
        return Err(err(name, "patterns and pattern_items must be arrays"));
    };
    if patterns_raw.len() != pattern_items_raw.len() {
        return Err(err(name, "patterns and pattern_items differ in length"));
    }

    let item_count = items.len();
    let mut parts = Parts::new(groups, items, patterns_raw.len());
    for (p, ids) in patterns_raw.into_iter().zip(pattern_items_raw) {
        let (group, text) = match p {
            Value::Array(pair) if pair.len() == 2 => {
                let mut it = pair.into_iter();
                match (it.next(), it.next()) {
                    (Some(Value::String(g)), Some(Value::String(t))) => (g, t),
                    _ => return Err(err(name, "pattern must be [group, text]")),
                }
            }
            _ => return Err(err(name, "pattern must be [group, text]")),
        };
        let Some(gi) = parts.groups.get_index_of(&group) else {
            return Err(err(name, &format!("pattern references unknown group: {group}")));
        };
        let ids = match ids {
            Value::Array(ids) if !ids.is_empty() => ids,
            _ => return Err(err(name, "pattern_items entry must be a non-empty array")),
        };
        let start = parts.item_ids.len();
        for id in &ids {
            match id {
                Value::Int(i) if *i >= 0 && (*i as u64) < item_count as u64 => parts.item_ids.push(*i as u32),
                _ => return Err(err(name, "pattern_items references unknown item")),
            }
        }
        if parts.item_ids[start..].windows(2).any(|w| w[0] >= w[1]) {
            return Err(err(name, "pattern_items entry must be sorted and unique"));
        }
        parts.item_offsets.push(parts.item_ids.len() as u32);
        parts.pattern_group.push(gi as u32);
        parts.pattern_text.push(text);
    }
    Ok(parts)
}

impl Index {
    /// Builds an index from its parsed `index/<name>` payload.
    pub fn new(name: &str, payload: Value, loader: AutomatonLoader) -> Result<Index> {
        Ok(Index::from_parts(name, parse_payload(name, payload)?, loader))
    }

    /// Builds an index from the raw bytes of its `index/<name>` section.
    pub fn from_json(name: &str, raw: &[u8], loader: AutomatonLoader) -> Result<Index> {
        if let Some(parts) = parse_canonical(raw) {
            return Ok(Index::from_parts(name, parts, loader));
        }
        let payload = crate::format::container::parse_json(raw, &format!("index {name}"))?;
        Index::new(name, payload, loader)
    }

    fn from_parts(name: &str, parts: Parts, loader: AutomatonLoader) -> Index {
        let group_count = parts.groups.len();
        Index {
            name: name.to_string(),
            groups: parts.groups,
            items: parts.items,
            pattern_group: parts.pattern_group,
            pattern_text: parts.pattern_text,
            item_offsets: parts.item_offsets,
            item_ids: parts.item_ids,
            loader,
            automata: (0..group_count).map(|_| OnceLock::new()).collect(),
            exact: (0..group_count).map(|_| OnceLock::new()).collect(),
            lengths: OnceLock::new(),
            key_ids: OnceLock::new(),
            sorted_keys: OnceLock::new(),
            group_counts: Mutex::new(HashMap::new()),
        }
    }

    pub fn pattern_total(&self) -> usize {
        self.pattern_text.len()
    }

    fn items_of(&self, pid: usize) -> &[u32] {
        &self.item_ids[self.item_offsets[pid] as usize..self.item_offsets[pid + 1] as usize]
    }

    fn group_name(&self, pid: usize) -> &str {
        self.groups.get_index(self.pattern_group[pid] as usize).map_or("", |(g, _)| g.as_str())
    }

    /// Text, group and owning items of a pattern.
    pub fn pattern(&self, pattern_id: usize) -> Option<Pattern> {
        (pattern_id < self.pattern_text.len()).then(|| Pattern {
            id: pattern_id,
            group: self.group_name(pattern_id).to_string(),
            text: self.pattern_text[pattern_id].clone(),
            item_ids: self.items_of(pattern_id).iter().map(|&i| i as usize).collect(),
        })
    }

    /// Text of a pattern without copying it.
    pub fn pattern_text(&self, pattern_id: usize) -> Option<&str> {
        self.pattern_text.get(pattern_id).map(String::as_str)
    }

    /// Group of a pattern without copying it.
    pub fn pattern_group(&self, pattern_id: usize) -> Option<&str> {
        (pattern_id < self.pattern_text.len()).then(|| self.group_name(pattern_id))
    }

    /// Items that own a pattern.
    pub fn pattern_items(&self, pattern_id: usize) -> Option<impl Iterator<Item = usize> + '_> {
        (pattern_id < self.pattern_text.len()).then(|| self.items_of(pattern_id).iter().map(|&i| i as usize))
    }

    pub fn item(&self, key: &str) -> Option<&Item> {
        self.item_id(key).map(|i| &self.items[i])
    }

    pub fn item_id(&self, key: &str) -> Option<usize> {
        let ids = self
            .key_ids
            .get_or_init(|| self.items.iter().enumerate().map(|(i, item)| (item.key.clone(), i)).collect());
        ids.get(key).copied()
    }

    /// Items whose key starts with `prefix`, in key order.
    pub fn items_with_prefix(&self, prefix: &str) -> Vec<&Item> {
        let keys = self.sorted_keys.get_or_init(|| {
            let mut ids: Vec<usize> = (0..self.items.len()).collect();
            ids.sort_by(|&a, &b| self.items[a].key.cmp(&self.items[b].key).then(a.cmp(&b)));
            ids
        });
        let start = keys.partition_point(|&i| self.items[i].key.as_str() < prefix);
        keys[start..]
            .iter()
            .take_while(|&&i| self.items[i].key.starts_with(prefix))
            .map(|&i| &self.items[i])
            .collect()
    }

    /// Number of patterns of an item, in one group or (`None`) in all of them. `None` when the
    /// item id is out of range.
    pub fn pattern_count(&self, item_id: usize, group: Option<&str>) -> Option<usize> {
        let key = group.map(str::to_string);
        let counts = {
            let mut cache = self.group_counts.lock().unwrap_or_else(|e| e.into_inner());
            cache
                .entry(key)
                .or_insert_with(|| {
                    let mut counts = vec![0usize; self.items.len()];
                    for pid in 0..self.pattern_text.len() {
                        if group.is_none_or(|g| self.group_name(pid) == g) {
                            for &i in self.items_of(pid) {
                                counts[i as usize] += 1;
                            }
                        }
                    }
                    Arc::new(counts)
                })
                .clone()
        };
        counts.get(item_id).copied()
    }

    /// Builds every lookup now instead of on first use.
    pub fn preload(&self) -> Result<()> {
        for (gi, spec) in self.groups.values().enumerate() {
            if spec.match_mode == MatchMode::Exact {
                self.exact_map(gi);
            } else {
                self.automaton(gi)?;
            }
        }
        self.pattern_lengths();
        Ok(())
    }

    pub fn spec(&self, group: &str) -> Result<&GroupSpec> {
        self.groups.get(group).ok_or_else(|| Error::format(format!("unknown group: {group}")))
    }

    fn group_index(&self, spec: &GroupSpec) -> usize {
        self.groups.get_index_of(&spec.name).unwrap_or(0)
    }

    fn automaton(&self, gi: usize) -> Result<Option<&Automaton>> {
        let slot = self.automata[gi].get_or_init(|| {
            let group = self.groups.get_index(gi).map_or("", |(g, _)| g.as_str());
            (self.loader)(group, self.pattern_text.len())
        });
        match slot {
            Ok(a) => Ok(a.as_ref()),
            Err(e) => Err(e.clone()),
        }
    }

    fn pattern_lengths(&self) -> &[usize] {
        self.lengths.get_or_init(|| self.pattern_text.iter().map(String::len).collect())
    }

    fn exact_map(&self, gi: usize) -> &HashTable<u32> {
        self.exact[gi].get_or_init(|| {
            let texts = &self.pattern_text;
            let mut table: HashTable<u32> = HashTable::new();
            for (pid, text) in texts.iter().enumerate() {
                if self.pattern_group[pid] as usize != gi {
                    continue;
                }
                // A later pattern with the same text replaces an earlier one, as in a dict.
                let h = hash_str(text);
                match table.entry(h, |&p| texts[p as usize] == *text, |&p| hash_str(&texts[p as usize])) {
                    hashbrown::hash_table::Entry::Occupied(mut e) => *e.get_mut() = pid as u32,
                    hashbrown::hash_table::Entry::Vacant(e) => {
                        e.insert(pid as u32);
                    }
                }
            }
            table
        })
    }

    fn exact_lookup(&self, gi: usize, head: &str) -> Option<u32> {
        let texts = &self.pattern_text;
        self.exact_map(gi).find(hash_str(head), |&p| texts[p as usize] == head).copied()
    }

    fn find(&self, spec: &GroupSpec, head: &str) -> Result<Vec<(usize, u32)>> {
        let gi = self.group_index(spec);
        if spec.match_mode == MatchMode::Exact {
            return Ok(self.exact_lookup(gi, head).map(|pid| vec![(head.len(), pid)]).unwrap_or_default());
        }
        let Some(automaton) = self.automaton(gi)? else {
            return Ok(Vec::new());
        };
        if spec.match_mode == MatchMode::Prefix {
            return Ok(automaton.find_prefix(head.as_bytes(), self.pattern_lengths()));
        }
        Ok(automaton.find_contains(head.as_bytes()))
    }

    fn first(&self, spec: &GroupSpec, head: String) -> Result<MatchResult<'_>> {
        let found = self.find(spec, &head)?;
        let Some(end) = found.iter().map(|&(e, _)| e).min() else {
            return Ok(MatchResult::none(head));
        };
        let (item_id, pid) = found
            .iter()
            .filter(|&&(e, _)| e == end)
            .map(|&(_, pid)| (self.items_of(pid as usize)[0] as usize, pid as usize))
            .min()
            .expect("at least one match ends at the minimum");
        Ok(MatchResult {
            result: true,
            item_id: Some(item_id),
            item: Some(&self.items[item_id]),
            head,
            pattern_id: Some(pid),
        })
    }

    fn heads_one(&self, spec: &GroupSpec, value: &str, name: Option<&str>) -> Result<String> {
        if spec.kind == GroupKind::Map {
            let Some(name) = name else {
                return Err(Error::format(format!("group {} requires a name", spec.name)));
            };
            return Ok(normalize(spec, &format_map_value(name, value)));
        }
        if name.is_some() {
            return Err(Error::format(format!("group {} does not accept a name", spec.name)));
        }
        Ok(normalize(spec, value))
    }

    fn heads(&self, spec: &GroupSpec, values: &Values<'_>) -> Result<Vec<String>> {
        let no_name = || Error::format(format!("group {} does not accept a name", spec.name));
        let raw: Vec<String> = match values {
            Values::One(v) => return Ok(vec![normalize(spec, v)]),
            Values::Many(v) => v.iter().map(|s| s.to_string()).collect(),
            Values::Pairs(pairs) => {
                if spec.kind != GroupKind::Map {
                    return Err(no_name());
                }
                pairs.iter().map(|(k, v)| format_map_value(k, v)).collect()
            }
            Values::Json(Value::String(v)) => return Ok(vec![normalize(spec, v)]),
            Values::Json(v @ Value::Object(_)) => {
                if spec.kind != GroupKind::Map {
                    return Err(no_name());
                }
                parse_values(spec, Some(v))?
            }
            Values::Json(Value::Array(items)) => items
                .iter()
                .map(|v| match v {
                    Value::String(s) => Ok(s.clone()),
                    _ => Err(Error::format(format!("{} values must be strings", spec.name))),
                })
                .collect::<Result<_>>()?,
            Values::Json(_) => {
                return Err(Error::format(format!("{} values must be a string, list, or object", spec.name)));
            }
        };
        Ok(raw.iter().map(|v| normalize(spec, v)).collect())
    }

    /// First match of a header line (`Name: value`) in the `headers` group.
    pub fn r#match(&self, head: &str) -> Result<MatchResult<'_>> {
        let spec = self.spec("headers")?;
        self.first(spec, normalize(spec, head))
    }

    /// Alias of `r#match`.
    pub fn match_head(&self, head: &str) -> Result<MatchResult<'_>> {
        self.r#match(head)
    }

    /// First match of one value in a group; map groups take the name separately.
    pub fn match_group(&self, group: &str, value: &str, name: Option<&str>) -> Result<MatchResult<'_>> {
        let spec = self.spec(group)?;
        let head = self.heads_one(spec, value, name)?;
        self.first(spec, head)
    }

    /// First match of a search object `{group: values}`, trying groups and values in order.
    pub fn match_search(&self, search: &Value) -> Result<MatchResult<'_>> {
        let Value::Object(search) = search else {
            return Err(Error::format("search must be an object"));
        };
        for (group, values) in search {
            let spec = self.spec(group)?;
            for value in parse_values(spec, Some(values))? {
                let result = self.first(spec, normalize(spec, &value))?;
                if result.result {
                    return Ok(result);
                }
            }
        }
        Ok(MatchResult::none(""))
    }

    /// First match of the tags and attributes of an HTML document in the `html` group.
    pub fn match_html(&self, html: &str) -> Result<MatchResult<'_>> {
        let spec = self.spec("html")?;
        for head in html_heads(html) {
            let result = self.first(spec, normalize(spec, &head))?;
            if result.result {
                return Ok(result);
            }
        }
        Ok(MatchResult::none(""))
    }

    /// Every item with at least one matching pattern, by number of distinct patterns hit.
    pub fn match_all<'v>(&self, group: &str, values: impl Into<Values<'v>>) -> Result<Vec<Hit<'_>>> {
        let spec = self.spec(group)?;
        let heads = self.heads(spec, &values.into())?;
        let mut pattern_ids = Vec::new();
        for head in &heads {
            pattern_ids.extend(self.find(spec, head)?.into_iter().map(|(_, pid)| pid as usize));
        }
        self.hits_for(pattern_ids)
    }

    /// `{item_id: distinct patterns hit}` for a list of tokens, in `match_all` order.
    pub fn match_tokens<'v>(
        &self,
        group: &str,
        tokens: impl Into<Values<'v>>,
    ) -> Result<IndexMap<usize, usize>> {
        let tokens = tokens.into();
        if matches!(tokens, Values::One(_) | Values::Json(Value::String(_))) {
            return Err(Error::format("tokens must be a list of strings"));
        }
        let spec = self.spec(group)?;
        if spec.match_mode == MatchMode::Exact && !matches!(tokens, Values::Pairs(_) | Values::Json(_)) {
            return self.match_tokens_exact(spec, &tokens);
        }
        Ok(self.match_all(group, tokens)?.into_iter().map(|h| (h.item_id, h.hits)).collect())
    }

    fn match_tokens_exact(&self, spec: &GroupSpec, tokens: &Values<'_>) -> Result<IndexMap<usize, usize>> {
        let Values::Many(list) = tokens else { unreachable!() };
        let gi = self.group_index(spec);
        let mut pids: Vec<u32> = Vec::with_capacity(list.len());
        for t in list {
            if let Some(pid) = self.exact_lookup(gi, &normalize(spec, t)) {
                pids.push(pid);
            }
        }
        pids.sort_unstable();
        pids.dedup();
        let mut per_item: FxHashMap<u32, usize> = FxHashMap::default();
        for &pid in &pids {
            for &i in self.items_of(pid as usize) {
                *per_item.entry(i).or_insert(0) += 1;
            }
        }
        let mut hits: Vec<(usize, usize)> = per_item.into_iter().map(|(i, n)| (i as usize, n)).collect();
        hits.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Ok(hits.into_iter().collect())
    }

    /// Every occurrence of the group's patterns in `text` (lowercased first when the group
    /// ignores case), with UTF-8 byte offsets.
    pub fn scan(&self, text: &str, group: &str) -> Result<Vec<Occurrence>> {
        let spec = self.spec(group)?;
        if spec.match_mode == MatchMode::Exact {
            return Err(Error::format(format!("group {group} uses exact match and cannot be scanned")));
        }
        let Some(automaton) = self.automaton(self.group_index(spec))? else {
            return Ok(Vec::new());
        };
        let lowered;
        let data = if spec.ignore_case {
            lowered = lower(text);
            lowered.as_bytes()
        } else {
            text.as_bytes()
        };
        let lengths = self.pattern_lengths();
        let mut out = Vec::new();
        automaton.for_each_contains(data, |end, pid| {
            let pid = pid as usize;
            out.push(Occurrence { pattern_id: pid, start: end - lengths[pid], end });
        });
        Ok(out)
    }

    /// Groups pattern ids by item, ordered by hit count, then item id.
    pub fn hits_for(&self, pattern_ids: impl IntoIterator<Item = usize>) -> Result<Vec<Hit<'_>>> {
        let mut pids: Vec<usize> = pattern_ids.into_iter().collect();
        pids.sort_unstable();
        pids.dedup();
        let mut per_item: IndexMap<usize, Vec<usize>> = IndexMap::new();
        for pid in pids {
            if pid >= self.pattern_text.len() {
                return Err(Error::format(format!("unknown pattern id: {pid}")));
            }
            for &i in self.items_of(pid) {
                per_item.entry(i as usize).or_default().push(pid);
            }
        }
        let mut hits: Vec<Hit<'_>> = per_item
            .into_iter()
            .map(|(i, pids)| Hit { item_id: i, item: &self.items[i], hits: pids.len(), pattern_ids: pids })
            .collect();
        hits.sort_by(|a, b| b.hits.cmp(&a.hits).then(a.item_id.cmp(&b.item_id)));
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: &Index, b: &Index) {
        assert_eq!(a.groups, b.groups);
        assert_eq!(a.items, b.items);
        assert_eq!(a.pattern_group, b.pattern_group);
        assert_eq!(a.pattern_text, b.pattern_text);
        assert_eq!(a.item_offsets, b.item_offsets);
        assert_eq!(a.item_ids, b.item_ids);
    }

    fn none() -> AutomatonLoader {
        Box::new(|_, _| Ok(None))
    }

    #[test]
    fn canonical_and_generic_payloads_agree() {
        let rules = Value::from_json(
            r#"{"10": {"features": ["a", "b"], "data": {"x": [1, 2.5, null, {"y": "z"}]}},
                "2": {"features": ["b"], "strings": "needle", "headers": {"Server": "nginx"}},
                "\u0000k": {"html": [{"tag": "div", "attr": "id"}, "attr:x"], "data": " "}}"#,
        )
        .unwrap();
        let groups = Value::from_json(r#"{"features": {}, "strings": {"match": "contains"}}"#).unwrap();
        let sections = crate::compiler::compile_index("main", &rules, Some(&groups)).unwrap();
        let raw = &sections[0].1;
        assert!(parse_canonical(raw).is_some());
        let fast = Index::from_json("main", raw, none()).unwrap();
        let slow = Index::new("main", crate::json::parse_bytes(raw).unwrap(), none()).unwrap();
        same(&fast, &slow);

        // Other spellings of the same payload take the generic path and give the same index.
        let pretty = crate::json::parse_bytes(raw).unwrap();
        let mut spaced = String::from(" ");
        spaced.push_str(&pretty.to_json().unwrap().replace(",", ", ").replace(":[", ": ["));
        assert!(parse_canonical(spaced.as_bytes()).is_none());
        same(&Index::from_json("main", spaced.as_bytes(), none()).unwrap(), &slow);
        let mut reordered = pretty.as_object().unwrap().clone();
        let groups = reordered.shift_remove("groups").unwrap();
        reordered.insert("groups".into(), groups);
        reordered.insert("extra".into(), Value::Null);
        let text = Value::Object(reordered).to_json().unwrap();
        assert!(parse_canonical(text.as_bytes()).is_none());
        same(&Index::from_json("main", text.as_bytes(), none()).unwrap(), &slow);
    }

    #[test]
    fn canonical_path_defers_on_invalid_content() {
        let bad = [
            r#"{"groups":{"g":{"kind":"list","match":"exact","ignore_case":false,"trim":false}},"items":[["a",null]],"patterns":[["g","x"]],"pattern_items":[[1]]}"#,
            r#"{"groups":{"g":{"kind":"list","match":"exact","ignore_case":false,"trim":false}},"items":[["a",null]],"patterns":[["h","x"]],"pattern_items":[[0]]}"#,
            r#"{"groups":{"g":{"kind":"list","match":"exact","ignore_case":false,"trim":false}},"items":[["a",null],["b",1]],"patterns":[["g","x"]],"pattern_items":[[1,0]]}"#,
            r#"{"groups":{"g":{"kind":"list","match":"exact","ignore_case":false,"trim":false}},"items":[["a",null]],"patterns":[["g","x"]],"pattern_items":[[]]}"#,
            r#"{"groups":{"g":{"kind":"list","match":"exact","ignore_case":false,"trim":false}},"items":[["a",null]],"patterns":[["g","x"]],"pattern_items":[[0.0]]}"#,
            r#"{"groups":{"g":{"kind":"list","match":"exact","ignore_case":false,"trim":false}},"items":[["",null]],"patterns":[],"pattern_items":[]}"#,
            r#"{"groups":{"g":{"kind":"nope","match":"exact","ignore_case":false,"trim":false}},"items":[],"patterns":[],"pattern_items":[]}"#,
        ];
        for text in bad {
            assert!(parse_canonical(text.as_bytes()).is_none(), "{text}");
            let generic = Index::new("main", Value::from_json(text).unwrap(), none()).unwrap_err();
            assert_eq!(Index::from_json("main", text.as_bytes(), none()).unwrap_err(), generic);
        }
    }
}
