# SignatureDB

[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green)](#license)
[![Format](https://img.shields.io/badge/format-SIGT%20v3-lightgrey)](#file-format)

Compiler and loader for `.sigdb` files: rules compiled into per-group Aho-Corasick indexes,
with arbitrary JSON per item and named data sections, packed into one zstd-compressed file.
Used for technology fingerprinting (headers, HTML, scripts) and for library fingerprinting
(feature tokens, source strings).

A port of [reekeer/sigdb](https://github.com/reekeer/sigdb) (Python) with the same behaviour as
[reekeer/sigdb-ts](https://github.com/reekeer/sigdb-ts): the same rules give the same file, byte for
byte, and every query gives the same answer and the same error message.

## Install

```toml
[dependencies]
sigdb = { git = "https://github.com/reekeer/sigdb-rs" }
```

Requires Rust 1.85+. zstd is the C library (built from source by `zstd-sys`), the one
python-zstandard bundles too.

## Usage

```rust
use sigdb::{BuildOptions, Value, build, load};

let rules = Value::from_json(r#"{
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
}"#)?;
let options = BuildOptions::new()
    .groups(Value::from_json(r#"{"features": {}, "strings": {"match": "contains"}}"#)?)
    .section("prints", Value::from_json(r#"{"version": 1}"#)?);
build(Some(&rules), "libs.sigdb", &options)?;

let db = load("libs.sigdb")?;
db.r#match("Server: nginx/1.25.3")?.key();                 // Some("nginx")
db.match_tokens("features", &["P:useState", "K:subscribe"])?; // {2: 2, 1: 1}
db.match_all("features", &["P:useState"])?;                 // [Hit { item_id: 1, hits: 1, .. }, ..]
db.scan(source_code, "strings")?;                           // [Occurrence { pattern_id, start, end }]
db.item("react@18.2.0/index.js")?.unwrap().data;           // {"package": "react", ...}
db.items_with_prefix("react@")?;
db.section("prints")?;                                      // Section::Json({"version": 1})
```

Other entry points: `build_bytes`, `load_bytes`, `load_with` (`LoadOptions { verify_hash, lazy,
max_section_size }`), `compile_json`, `compile_dir` (merges every `*.json` under a directory),
`read_rules`, `read_metadata`, `validate`, `Reader`, `Database::from_raw` / `raw_sections`.
Names follow the Python package; `match` is a Rust keyword, so it is `r#match` (alias
`match_head`).

Rules, group configs, indexes and metadata are `Value`s: JSON with Python's data model (object
keys keep their order, integers of any size stay exact, `1` and `1.0` differ). Parse them with
`Value::from_json`, collect them from iterators, or convert a `serde_json::Value` (the `serde`
feature, on by default, also implements `Serialize`/`Deserialize`).

A `Database` decodes an index or section on first use and keeps it; it is `Send + Sync`, so one
instance can serve many threads behind an `Arc`. Query results borrow the database: `MatchResult`
and `Hit` hold `&Item`.

## Rules

A rule set is a JSON object: item key to its groups. Keys are any non-empty strings
(`pkg@1.0.0/dist/a.js`, `/`, `@` and `\u0000` included). `data` holds any JSON for the item and
is never matched.

Every group has a config:

| field | values | meaning |
|---|---|---|
| `match` | `prefix`, `contains`, `exact` | value starts with / contains / equals the pattern |
| `ignore_case` | bool | lowercase patterns and queries |
| `trim` | bool | strip surrounding whitespace |
| `kind` | `list`, `map` | `map` groups take `{name: value}` and match `name:value` |

Built-in groups: `headers` (map, contains), `meta` (map, prefix), and `js`, `html`,
`script_src`, `css`, `url`, `path`, `file`, `dns`, `subdomain`, `link`, `json`, `api`, `tls`,
`server`, `framework`, `cms`, `cdn` (list, prefix). All built-ins ignore case and trim.
Custom groups are declared in `groups` and default to exact, case-sensitive, no trim.

A file can hold several indexes (`BuildOptions::index("functions", rules, groups)`, read with
`db.index("functions")`). Plain rules go to the `main` index.

Queries:

- `r#match`, `match_group`, `match_search`, `match_html` return the first hit: earliest match,
  then lowest item id.
- `match_all` and `match_tokens` return every item with the number of distinct patterns hit.
- `scan` returns every occurrence with UTF-8 byte offsets (of the lowercased text when the
  group ignores case).
- `pattern(id)` and `pattern_count(item_id, group)` give pattern text and totals for weighting.

Builds are reproducible: no timestamps unless `timestamp` is set.

## Same as Python

Lowercasing, whitespace and the HTML tag/attribute expressions follow Python's `str.lower()`,
`str.strip()` and `re` (`\w`, `\s`, `\b`, `re.IGNORECASE`) for the Unicode version of the Python
the tables were generated from (3.13, Unicode 15.1): characters added to Unicode later lower to
themselves, as they do in Python, although the Rust standard library already knows them.
`scripts/gen_tables.py` regenerates `src/tables.rs`.

JSON is read and written like Python's `json` module: `NaN`/`Infinity` literals are read (and
rejected when writing), floats print as `float.__repr__` (`1e+16`, `1e-05`, `100.0`), bytes in
UTF-16/32 are detected, the last duplicate key wins in the position of the first.

Differences, all on input the Python package cannot write either: a lone UTF-16 surrogate
(`"\ud800"`) cannot exist in a Rust string, so JSON holding one is invalid; varints wider than
64 bits are read as `u64::MAX`, which fails the same bounds checks; automata whose fail links do
not lead to a shallower state are rejected (as sigdb-ts does) instead of looping.

## File format

```
"SIGT" | u8 version = 3 | u32 header length | header JSON
       | u16 section count | per section: u8 name length, name, u32 raw size,
         u32 stored size, SHA256(raw) | zstd section bodies
```

Integers are big-endian. Sections are `index/<name>` (JSON: groups, items, patterns,
pattern_items), `automaton/<index>/<group>`, `json/<name>` and `blob/<name>`. Each section
is decompressed and hash-checked on first use. Versions 1 and 2 are rejected; rebuild
them from rules. Specification: [reekeer/sigdb](https://github.com/reekeer/sigdb).

## Performance

On unbundle's `fingerprints.sigdb` (3 indexes, 47 800 items, 251 000 patterns, 23 MB of index
JSON) and a 4-core machine:

| | Python 3.13 | Rust |
|---|---|---|
| load and decode every index | 2.65 s | 0.2 s |
| `match_tokens`, 300 tokens | 2.5 ms | 0.09 ms |
| build 25 000 `contains` patterns | 3.6 s | 0.96 s |
| `scan` 1 MB of text | 0.78 s | 27 ms |
| rebuild the whole file (zstd 19) | 14.1 s | 8.4 s |

A full rebuild is bound by zstd at level 19 (the same C library in both); sections are
compressed in parallel, each in a single thread, so the output is unchanged.
`cargo run --release --example bench -- <file.sigdb>` prints these numbers for any file.

## Tests

```sh
cargo test
```

- `tests/golden/` (from sigdb) and `tests/golden-extra/` (from sigdb-ts): rule sets with
  expected results, section hashes and build errors.
- `tests/fixtures/py/` and `tests/fixtures/ts/`: files built by the Python package and by
  sigdb-ts. Rust answers every vector on them, and its own build of the same rules is
  byte-identical to the Python file.
- `tests/fixtures/html_heads.json`, `tests/fixtures/repr.json`: reference output of Python's
  regular expressions and `repr()`.

`cargo run --example fixtures -- <dir>` writes the Rust build of every golden case; sigdb and
sigdb-ts keep these files in `tests/fixtures/rs/` and check them in their own test suites.

Differential check against random rule sets (Unicode case and whitespace, map groups, HTML,
floats, big integers, key order, invalid input):

```sh
pip install sigdb==3.0.0
python scripts/crosscheck.py /tmp/cross --cases 2000
cargo run --release --example crosscheck -- /tmp/cross

python scripts/crosscheck.py /tmp/cross-ts --cases 2000 --no-new-unicode
cargo run --release --example crosscheck -- /tmp/cross-ts
SIGDB_TS=../sigdb-ts bun scripts/crosscheck-ts.ts /tmp/cross-ts
cargo run --release --example crosscheck -- /tmp/cross-ts
```

The first run requires the Rust build to be byte-identical to Python's, the same build errors,
and Python's answer to every query on both files. The second also has sigdb-ts answer every
query on the Python and Rust files, then Rust read the files sigdb-ts built. `--no-new-unicode`
leaves out characters added after Unicode 15.1, which JavaScript engines lowercase where Python
does not.

## License

MIT
