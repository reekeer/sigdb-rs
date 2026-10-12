use std::path::Path;
use std::sync::{Arc, OnceLock};

use indexmap::IndexMap;

use crate::compiler::{DEFAULT_INDEX, automaton_section, index_section};
use crate::error::{Error, Result};
use crate::format::automaton::deserialize_automaton;
use crate::format::container::{DEFAULT_MAX_SECTION_SIZE, RawSections, Store, parse_container, parse_json};
use crate::groups::MatchMode;
use crate::index::{Index, Values};
use crate::types::{Hit, Item, MatchResult, Occurrence, Section, ValidationResult};
use crate::value::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadOptions {
    pub verify_hash: bool,
    pub lazy: bool,
    pub max_section_size: u64,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions { verify_hash: true, lazy: true, max_section_size: DEFAULT_MAX_SECTION_SIZE }
    }
}

pub struct Database {
    pub metadata: Map,
    store: Arc<Store>,
    indexes: IndexMap<String, OnceLock<Result<Index>>>,
    sections: IndexMap<String, OnceLock<Result<Section>>>,
    section_order: Vec<String>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("metadata", &self.metadata)
            .field("indexes", &self.index_names())
            .field("sections", &self.section_names())
            .finish()
    }
}

impl Database {
    fn new(metadata: Map, store: Store) -> Result<Database> {
        let mut indexes = IndexMap::new();
        let mut sections = IndexMap::new();
        let mut section_order = Vec::new();
        for name in store.names() {
            let (kind, rest) = name.split_once('/').unwrap_or((name, ""));
            if !matches!(kind, "index" | "automaton" | "json" | "blob") || rest.is_empty() {
                return Err(Error::format(format!("unknown section: {name}")));
            }
            match kind {
                "index" => {
                    indexes.insert(rest.to_string(), OnceLock::new());
                }
                "json" | "blob" => {
                    section_order.push(rest.to_string());
                    sections.entry(rest.to_string()).or_insert_with(OnceLock::new);
                }
                _ => {}
            }
        }
        if indexes.is_empty() {
            return Err(Error::format("database has no indexes"));
        }
        Ok(Database { metadata, store: Arc::new(store), indexes, sections, section_order })
    }

    pub fn from_raw(metadata: Map, sections: IndexMap<String, Vec<u8>>) -> Result<Database> {
        Database::new(metadata, Store::Raw(RawSections { raw: sections }))
    }

    pub fn raw_sections(&self) -> Result<IndexMap<String, Vec<u8>>> {
        let mut out = IndexMap::new();
        for name in self.store.names() {
            out.insert(name.to_string(), self.store.raw(name)?.into_owned());
        }
        Ok(out)
    }

    pub fn section_digests(&self) -> IndexMap<String, String> {
        self.store.digests()
    }

    pub fn index_names(&self) -> Vec<&str> {
        self.indexes.keys().map(String::as_str).collect()
    }

    pub fn section_names(&self) -> Vec<&str> {
        self.section_order.iter().map(String::as_str).collect()
    }

    pub fn section(&self, name: &str) -> Result<&Section> {
        let slot =
            self.sections.get(name).ok_or_else(|| Error::format(format!("missing section: {name}")))?;
        let value = slot.get_or_init(|| {
            let json_name = format!("json/{name}");
            if self.store.contains(&json_name) {
                let raw = self.store.raw(&json_name)?;
                Ok(Section::Json(parse_json(&raw, &format!("section {name}"))?))
            } else {
                Ok(Section::Blob(self.store.raw(&format!("blob/{name}"))?.into_owned()))
            }
        });
        value.as_ref().map_err(Clone::clone)
    }

    pub fn index(&self, name: &str) -> Result<&Index> {
        let slot = self.indexes.get(name).ok_or_else(|| Error::format(format!("missing index: {name}")))?;
        let index = slot.get_or_init(|| {
            let raw = self.store.raw(&index_section(name))?;
            let store = Arc::clone(&self.store);
            let index_name = name.to_string();
            let loader = Box::new(move |group: &str, pattern_count: usize| {
                let section = automaton_section(&index_name, group);
                if !store.contains(&section) {
                    return Ok(None);
                }
                deserialize_automaton(&store.raw(&section)?, pattern_count).map(Some)
            });
            Index::from_json(name, &raw, loader)
        });
        index.as_ref().map_err(Clone::clone)
    }

    pub fn main(&self) -> Result<&Index> {
        self.index(DEFAULT_INDEX)
    }

    pub fn items(&self) -> Result<&[Item]> {
        Ok(&self.main()?.items)
    }

    pub fn item(&self, key: &str) -> Result<Option<&Item>> {
        Ok(self.main()?.item(key))
    }

    pub fn items_with_prefix(&self, prefix: &str) -> Result<Vec<&Item>> {
        Ok(self.main()?.items_with_prefix(prefix))
    }

    pub fn r#match(&self, head: &str) -> Result<MatchResult<'_>> {
        self.main()?.r#match(head)
    }

    pub fn match_head(&self, head: &str) -> Result<MatchResult<'_>> {
        self.main()?.r#match(head)
    }

    pub fn match_group(&self, group: &str, value: &str, name: Option<&str>) -> Result<MatchResult<'_>> {
        self.main()?.match_group(group, value, name)
    }

    pub fn match_search(&self, search: &Value) -> Result<MatchResult<'_>> {
        self.main()?.match_search(search)
    }

    pub fn match_html(&self, html: &str) -> Result<MatchResult<'_>> {
        self.main()?.match_html(html)
    }

    pub fn match_all<'v>(&self, group: &str, values: impl Into<Values<'v>>) -> Result<Vec<Hit<'_>>> {
        self.main()?.match_all(group, values)
    }

    pub fn match_tokens<'v>(
        &self,
        group: &str,
        tokens: impl Into<Values<'v>>,
    ) -> Result<IndexMap<usize, usize>> {
        self.main()?.match_tokens(group, tokens)
    }

    pub fn scan(&self, text: &str, group: &str) -> Result<Vec<Occurrence>> {
        self.main()?.scan(text, group)
    }

    fn load_all(&self) -> Result<()> {
        for name in self.indexes.keys() {
            self.index(name)?.preload()?;
        }
        for name in &self.section_order {
            self.section(name)?;
        }
        Ok(())
    }
}

pub fn load_bytes(data: impl Into<Vec<u8>>) -> Result<Database> {
    load_bytes_with(data, &LoadOptions::default())
}

pub fn load_bytes_with(data: impl Into<Vec<u8>>, options: &LoadOptions) -> Result<Database> {
    let container = parse_container(data.into(), options.verify_hash, options.max_section_size)?;
    let header = container.header.clone();
    let db = Database::new(header, Store::Container(container))?;
    if !options.lazy {
        db.load_all()?;
    }
    Ok(db)
}

pub fn load(path: impl AsRef<Path>) -> Result<Database> {
    load_with(path, &LoadOptions::default())
}

pub fn load_with(path: impl AsRef<Path>, options: &LoadOptions) -> Result<Database> {
    let data = read_file(path.as_ref())?;
    load_bytes_with(data, options)
}

pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| io_error(&e, path))
}

pub(crate) fn io_error(e: &std::io::Error, path: &Path) -> Error {
    let text = e.to_string();
    let message = match e.raw_os_error() {
        Some(code) => {
            let reason = text.split(" (os error").next().unwrap_or(&text);
            format!("[Errno {code}] {reason}: {}", crate::text::repr_str(&path.to_string_lossy()))
        }
        None => text,
    };
    Error::Io(e.kind(), message)
}

pub fn read_metadata(path: impl AsRef<Path>) -> Result<Map> {
    let path = path.as_ref();
    let file = std::fs::File::open(path).map_err(|e| io_error(&e, path))?;
    crate::format::container::read_header(std::io::BufReader::new(file))
}

pub fn validate(path: impl AsRef<Path>) -> ValidationResult {
    validate_with(path, true, DEFAULT_MAX_SECTION_SIZE)
}

pub fn validate_with(path: impl AsRef<Path>, verify_hash: bool, max_section_size: u64) -> ValidationResult {
    let mut errors = Vec::new();
    let mut metadata = None;
    let mut digests = None;
    let mut run = || -> Result<()> {
        let options = LoadOptions { verify_hash, lazy: true, max_section_size };
        let db = load_with(path.as_ref(), &options)?;
        metadata = Some(db.metadata.clone());
        let found = db.section_digests();
        digests = Some(found.clone());
        db.raw_sections()?;
        db.load_all()?;
        for section in found.keys() {
            let Some(rest) = section.strip_prefix("automaton/") else { continue };
            let (index_name, group) = rest.split_once('/').unwrap_or((rest, ""));
            if !db.indexes.contains_key(index_name) {
                errors.push(format!("automaton section for unknown index: {section}"));
                continue;
            }
            let spec = db.index(index_name)?.groups.get(group);
            if spec.is_none_or(|s| s.match_mode == MatchMode::Exact) {
                errors.push(format!("unexpected automaton section: {section}"));
            }
        }
        Ok(())
    };
    if let Err(e) = run() {
        errors.push(e.message().to_string());
    }
    ValidationResult { ok: errors.is_empty(), errors, metadata, sections: digests }
}
