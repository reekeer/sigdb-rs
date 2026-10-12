use std::path::PathBuf;

use indexmap::IndexMap;

use crate::value::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub id: usize,
    pub group: String,
    pub text: String,
    pub item_ids: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchResult<'a> {
    pub result: bool,
    pub item_id: Option<usize>,
    pub item: Option<&'a Item>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct Hit<'a> {
    pub item_id: usize,
    pub item: &'a Item,
    pub hits: usize,
    pub pattern_ids: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Occurrence {
    pub pattern_id: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuildResult {
    pub output_path: PathBuf,
    pub size: usize,
    pub metadata: Map,
    pub sections: IndexMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationResult {
    pub ok: bool,
    pub errors: Vec<String>,
    pub metadata: Option<Map>,
    pub sections: Option<IndexMap<String, String>>,
}

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
