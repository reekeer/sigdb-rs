pub mod compiler;
pub mod compression;
pub mod database;
pub mod error;
pub mod format;
pub mod groups;
mod hash;
pub mod html;
pub mod index;
pub mod json;
pub mod reader;
mod tables;
pub mod text;
pub mod types;
pub mod utils;
pub mod value;

pub use compiler::{BuildOptions, DEFAULT_INDEX, build, build_bytes, compile_dir, compile_json, read_rules};
pub use database::{
    Database, LoadOptions, load, load_bytes, load_bytes_with, load_with, read_metadata, validate,
    validate_with,
};
pub use error::{Error, Result};
pub use groups::{GroupKind, GroupSpec, MatchMode};
pub use index::{AutomatonLoader, Index, Values};
pub use reader::Reader;
pub use types::{BuildResult, Hit, Item, MatchResult, Occurrence, Pattern, Section, ValidationResult};
pub use value::{Map, Value};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
