//! Compiler and loader for `.sigdb` files: rules compiled into per-group Aho-Corasick indexes,
//! with arbitrary JSON per item and named data sections, packed into one zstd-compressed file.
//!
//! A port of the Python [`sigdb`](https://github.com/reekeer/sigdb) package: the same file
//! format (SIGT v3), the same section bytes for the same rules, and the same results and error
//! messages for every query.
//!
//! ```
//! use sigdb::{BuildOptions, Value, build_bytes, load_bytes};
//!
//! let rules = Value::from_json(r#"{
//!     "nginx": {"headers": {"Server": "nginx"}},
//!     "react@18.2.0/index.js": {
//!         "data": {"package": "react", "version": "18.2.0"},
//!         "features": ["P:useState", "S:react.element"]
//!     }
//! }"#).unwrap();
//! let options = BuildOptions::new().group("features", Value::from_json("{}").unwrap());
//! let db = load_bytes(build_bytes(Some(&rules), &options).unwrap()).unwrap();
//!
//! assert_eq!(db.r#match("Server: nginx/1.25.3").unwrap().key(), Some("nginx"));
//! let hits = db.match_tokens("features", &["P:useState"]).unwrap();
//! assert_eq!(hits.get(&1), Some(&1));
//! ```

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

/// Version of this crate, equal to the Python and TypeScript packages it mirrors.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
