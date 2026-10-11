//! A file path with a database loaded on first use.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use indexmap::IndexMap;

use crate::compiler::DEFAULT_INDEX;
use crate::database::{Database, LoadOptions, load_with, read_metadata, validate_with};
use crate::error::Result;
use crate::format::container::DEFAULT_MAX_SECTION_SIZE;
use crate::index::{Index, Values};
use crate::types::{Hit, Item, MatchResult, Occurrence, Section, ValidationResult};
use crate::value::{Map, Value};

/// Reads a `.sigdb` file lazily: the header on demand, the database on first query.
#[derive(Debug)]
pub struct Reader {
    path: PathBuf,
    verify_hash: bool,
    db: OnceLock<Database>,
}

impl Reader {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Reader::with_verify(path, true)
    }

    pub fn with_verify(path: impl AsRef<Path>, verify_hash: bool) -> Self {
        Reader { path: path.as_ref().to_path_buf(), verify_hash, db: OnceLock::new() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn metadata(&self) -> Result<Map> {
        read_metadata(&self.path)
    }

    pub fn validate(&self) -> ValidationResult {
        validate_with(&self.path, self.verify_hash, DEFAULT_MAX_SECTION_SIZE)
    }

    /// Loads a fresh database.
    pub fn load(&self) -> Result<Database> {
        load_with(&self.path, &LoadOptions { verify_hash: self.verify_hash, ..LoadOptions::default() })
    }

    /// The database, loaded once.
    pub fn database(&self) -> Result<&Database> {
        if let Some(db) = self.db.get() {
            return Ok(db);
        }
        let db = self.load()?;
        Ok(self.db.get_or_init(|| db))
    }

    pub fn index(&self, name: &str) -> Result<&Index> {
        self.database()?.index(name)
    }

    pub fn main(&self) -> Result<&Index> {
        self.index(DEFAULT_INDEX)
    }

    pub fn section(&self, name: &str) -> Result<&Section> {
        self.database()?.section(name)
    }

    pub fn item(&self, key: &str) -> Result<Option<&Item>> {
        self.database()?.item(key)
    }

    pub fn items_with_prefix(&self, prefix: &str) -> Result<Vec<&Item>> {
        self.database()?.items_with_prefix(prefix)
    }

    pub fn r#match(&self, head: &str) -> Result<MatchResult<'_>> {
        self.database()?.r#match(head)
    }

    pub fn match_head(&self, head: &str) -> Result<MatchResult<'_>> {
        self.database()?.r#match(head)
    }

    pub fn match_group(&self, group: &str, value: &str, name: Option<&str>) -> Result<MatchResult<'_>> {
        self.database()?.match_group(group, value, name)
    }

    pub fn match_search(&self, search: &Value) -> Result<MatchResult<'_>> {
        self.database()?.match_search(search)
    }

    pub fn match_html(&self, html: &str) -> Result<MatchResult<'_>> {
        self.database()?.match_html(html)
    }

    pub fn match_all<'v>(&self, group: &str, values: impl Into<Values<'v>>) -> Result<Vec<Hit<'_>>> {
        self.database()?.match_all(group, values)
    }

    pub fn match_tokens<'v>(
        &self,
        group: &str,
        tokens: impl Into<Values<'v>>,
    ) -> Result<IndexMap<usize, usize>> {
        self.database()?.match_tokens(group, tokens)
    }

    pub fn scan(&self, text: &str, group: &str) -> Result<Vec<Occurrence>> {
        self.database()?.scan(text, group)
    }
}
