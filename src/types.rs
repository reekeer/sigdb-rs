//! Result types returned by queries and builds.

use std::path::PathBuf;

use indexmap::IndexMap;

use crate::value::{Map, Value};

/// One rule of an index: its key and the JSON stored with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
    pub data: Value,
}

/// A pattern of an index and the items that own it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub id: usize,
    pub group: String,
    pub text: String,
    pub item_ids: Vec<usize>,
}

/// First match of `match`, `match_group`, `match_search` and `match_html`.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchResult<'a> {
    pub result: bool,
    pub item_id: Option<usize>,
    pub item: Option<&'a Item>,
    /// The normalized value that was looked up.
    pub head: String,
    pub pattern_id: Option<usize>,
}

impl MatchResult<'_> {
    pub(crate) fn none(head: impl Into<String>) -> Self {
        MatchResult { result: false, item_id: None, item: None, head: head.into(), pattern_id: None }
    }

    pub fn key(&self) -> Option<&str> {
        self.item.as_ref().map(|i| i.key.as_str())
    }
}

/// An item hit by `match_all`: how many distinct patterns of it matched, and which.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit<'a> {
    pub item_id: usize,
    pub item: &'a Item,
    pub hits: usize,
    pub pattern_ids: Vec<usize>,
}

/// One occurrence found by `scan`, with UTF-8 byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Occurrence {
    pub pattern_id: usize,
    pub start: usize,
    pub end: usize,
}

/// What `build` wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildResult {
    pub output_path: PathBuf,
    pub size: usize,
    pub metadata: Map,
    /// SHA-256 (hex) of every raw section.
    pub sections: IndexMap<String, String>,
}

/// Outcome of `validate`.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationResult {
    pub ok: bool,
    pub errors: Vec<String>,
    pub metadata: Option<Map>,
    pub sections: Option<IndexMap<String, String>>,
}

/// Value of a named data section: JSON (`json/<name>`) or bytes (`blob/<name>`).
#[derive(Debug, Clone, PartialEq)]
pub enum Section {
    Json(Value),
    Blob(Vec<u8>),
}

impl Section {
    pub fn as_json(&self) -> Option<&Value> {
        match self {
            Section::Json(v) => Some(v),
            Section::Blob(_) => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Section::Blob(b) => Some(b),
            Section::Json(_) => None,
        }
    }
}

impl From<Value> for Section {
    fn from(v: Value) -> Self {
        Section::Json(v)
    }
}

impl From<Vec<u8>> for Section {
    fn from(v: Vec<u8>) -> Self {
        Section::Blob(v)
    }
}

impl From<&[u8]> for Section {
    fn from(v: &[u8]) -> Self {
        Section::Blob(v.to_vec())
    }
}
