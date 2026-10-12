use std::borrow::Cow;
use std::io::Read;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use indexmap::IndexMap;

use crate::compression::{compress_zstd, decompress_zstd};
use crate::error::{Error, Result};
use crate::json;
use crate::text::repr_str;
use crate::utils::hashing::{hex, sha256};
use crate::value::{Map, Value};

pub const MAGIC: &[u8; 4] = b"SIGT";
pub const VERSION: u8 = 3;

pub const MAX_HEADER_BYTES: usize = 65_536;
pub const MAX_SECTIONS: usize = 65_535;
pub const MAX_SECTION_NAME_BYTES: usize = 255;
pub const SHA256_SIZE: usize = 32;
pub const DEFAULT_MAX_SECTION_SIZE: u64 = 512 * 1024 * 1024;

pub fn check_version(version: u8) -> Result<()> {
    let legacy = match version {
        1 => Some("legacy signed format"),
        2 => Some("single-automaton format"),
        _ => None,
    };
    if let Some(what) = legacy {
        return Err(Error::format(format!(
            "unsupported sigdb version: {version} ({what}); rebuild the database from rules"
        )));
    }
    if version != VERSION {
        return Err(Error::format(format!("unsupported sigdb version: {version}")));
    }
    Ok(())
}

pub fn dump_json(value: &Value) -> Result<Vec<u8>> {
    json::dump(value)
        .map(String::into_bytes)
        .map_err(|e| Error::format(format!("value is not serializable as JSON: {}", e.0)))
}

pub fn parse_json(data: &[u8], what: &str) -> Result<Value> {
    json::parse_bytes(data).map_err(|_| Error::format(format!("invalid {what} json")))
}

fn parse_header(raw: &[u8]) -> Result<Map> {
    match parse_json(raw, "HEADER_DATA")? {
        Value::Object(m) => Ok(m),
        _ => Err(Error::format("HEADER_DATA must be an object")),
    }
}

fn compress_all(sections: &[(String, Vec<u8>)], level: i32) -> Vec<Result<Vec<u8>>> {
    let total: usize = sections.iter().map(|(_, raw)| raw.len()).sum();
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(sections.len());
    if workers <= 1 || total < 64 * 1024 {
        return sections.iter().map(|(_, raw)| compress_zstd(raw, level)).collect();
    }
    let mut order: Vec<usize> = (0..sections.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(sections[i].1.len()));
    let next = AtomicUsize::new(0);
    let mut out: Vec<Option<Result<Vec<u8>>>> = (0..sections.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&i) = order.get(k) else { break };
                        done.push((i, compress_zstd(&sections[i].1, level)));
                    }
                    done
                })
            })
            .collect();
        for h in handles {
            for (i, r) in h.join().expect("zstd worker panicked") {
                out[i] = Some(r);
            }
        }
    });
    out.into_iter().map(|r| r.expect("every section compressed")).collect()
}

pub fn write_container(
    header: &Map,
    sections: &[(String, Vec<u8>)],
    zstd_level: i32,
) -> Result<(Vec<u8>, IndexMap<String, String>)> {
    let header_raw = dump_json(&Value::Object(header.clone()))?;
    if header_raw.len() > MAX_HEADER_BYTES {
        return Err(Error::format("HEADER_DATA too large"));
    }
    if sections.len() > MAX_SECTIONS {
        return Err(Error::format("too many sections"));
    }
    let mut seen = std::collections::HashSet::new();
    for (name, _) in sections {
        if name.is_empty() || name.len() > MAX_SECTION_NAME_BYTES {
            return Err(Error::format(format!("invalid section name: {}", repr_str(name))));
        }
        if !seen.insert(name.as_str()) {
            return Err(Error::format(format!("duplicate section: {name}")));
        }
    }

    let bodies = compress_all(sections, zstd_level);
    let mut table = Vec::new();
    let mut hashes = IndexMap::new();
    let mut stored = Vec::with_capacity(sections.len());
    for ((name, raw), body) in sections.iter().zip(bodies) {
        let body = body?;
        if raw.len() > u32::MAX as usize || body.len() > u32::MAX as usize {
            return Err(Error::format(format!("section {name} too large for 32-bit length")));
        }
        let digest = sha256(raw);
        hashes.insert(name.clone(), hex(&digest));
        table.push(name.len() as u8);
        table.extend_from_slice(name.as_bytes());
        table.extend_from_slice(&(raw.len() as u32).to_be_bytes());
        table.extend_from_slice(&(body.len() as u32).to_be_bytes());
        table.extend_from_slice(&digest);
        stored.push(body);
    }

    let size = 9 + header_raw.len() + 2 + table.len() + stored.iter().map(Vec::len).sum::<usize>();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&(header_raw.len() as u32).to_be_bytes());
    out.extend_from_slice(&header_raw);
    out.extend_from_slice(&(sections.len() as u16).to_be_bytes());
    out.extend_from_slice(&table);
    for body in &stored {
        out.extend_from_slice(body);
    }
    Ok((out, hashes))
}

#[derive(Debug, Clone)]
pub struct SectionEntry {
    pub name: String,
    pub raw_size: u32,
    pub digest: [u8; 32],
    body: Range<usize>,
}

#[derive(Debug)]
pub struct Container {
    pub header: Map,
    data: Vec<u8>,
    entries: IndexMap<String, SectionEntry>,
    verify_hash: bool,
    max_section_size: u64,
}

impl Container {
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn entry(&self, name: &str) -> Option<&SectionEntry> {
        self.entries.get(name)
    }

    pub fn digests(&self) -> IndexMap<String, String> {
        self.entries.iter().map(|(n, e)| (n.clone(), hex(&e.digest))).collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub fn body(&self, name: &str) -> Option<&[u8]> {
        self.entries.get(name).map(|e| &self.data[e.body.clone()])
    }

    pub fn raw(&self, name: &str) -> Result<Vec<u8>> {
        let entry =
            self.entries.get(name).ok_or_else(|| Error::format(format!("missing section: {name}")))?;
        if entry.raw_size as u64 > self.max_section_size {
            return Err(Error::format(format!("section {name} exceeds max_section_size")));
        }
        let raw = decompress_zstd(&self.data[entry.body.clone()], entry.raw_size as usize)
            .map_err(|e| Error::format(format!("section {name}: {}", e.message())))?;
        if self.verify_hash && sha256(&raw) != entry.digest {
            return Err(Error::integrity(format!("corrupted database (hash mismatch in section {name})")));
        }
        Ok(raw)
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(Error::format("unexpected EOF"));
        }
        let chunk = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(chunk)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
}

fn read_header_from(c: &mut Cursor<'_>) -> Result<Map> {
    if c.take(4)? != MAGIC {
        return Err(Error::format("invalid magic"));
    }
    check_version(c.u8()?)?;
    let header_len = c.u32()? as usize;
    if header_len > MAX_HEADER_BYTES {
        return Err(Error::format("HEADER_DATA too large"));
    }
    parse_header(c.take(header_len)?)
}

pub fn parse_container(data: Vec<u8>, verify_hash: bool, max_section_size: u64) -> Result<Container> {
    let mut c = Cursor { data: &data, pos: 0 };
    let header = read_header_from(&mut c)?;

    let count = c.u16()?;
    let mut table: Vec<(String, u32, u32, [u8; 32])> = Vec::with_capacity(count as usize);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..count {
        let name_len = c.u8()? as usize;
        let name = std::str::from_utf8(c.take(name_len)?)
            .map_err(|_| Error::format("invalid section name"))?
            .to_string();
        if name.is_empty() || !seen.insert(name.clone()) {
            return Err(Error::format(format!("invalid or duplicate section name: {}", repr_str(&name))));
        }
        let raw_size = c.u32()?;
        let body_size = c.u32()?;
        let digest: [u8; 32] = c.take(SHA256_SIZE)?.try_into().expect("32 bytes");
        table.push((name, raw_size, body_size, digest));
    }

    let mut entries = IndexMap::with_capacity(table.len());
    for (name, raw_size, body_size, digest) in table {
        let start = c.pos;
        c.take(body_size as usize)?;
        entries.insert(name.clone(), SectionEntry { name, raw_size, digest, body: start..c.pos });
    }

    if c.pos != data.len() {
        return Err(Error::format("trailing data after last section"));
    }
    Ok(Container { header, data, entries, verify_hash, max_section_size })
}

pub fn read_header(mut r: impl Read) -> Result<Map> {
    fn exact(r: &mut impl Read, n: usize) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(n);
        r.take(n as u64).read_to_end(&mut buf)?;
        if buf.len() != n {
            return Err(Error::format("unexpected EOF"));
        }
        Ok(buf)
    }
    if exact(&mut r, 4)? != MAGIC {
        return Err(Error::format("invalid magic"));
    }
    check_version(exact(&mut r, 1)?[0])?;
    let len = exact(&mut r, 4)?;
    let header_len = u32::from_be_bytes([len[0], len[1], len[2], len[3]]) as usize;
    if header_len > MAX_HEADER_BYTES {
        return Err(Error::format("HEADER_DATA too large"));
    }
    parse_header(&exact(&mut r, header_len)?)
}

pub fn read_header_file(path: impl AsRef<Path>) -> Result<Map> {
    let f = std::fs::File::open(path)?;
    read_header(std::io::BufReader::new(f))
}

#[derive(Debug, Default)]
pub(crate) struct RawSections {
    pub raw: IndexMap<String, Vec<u8>>,
}

#[derive(Debug)]
pub(crate) enum Store {
    Container(Container),
    Raw(RawSections),
}

impl Store {
    pub fn names(&self) -> Vec<&str> {
        match self {
            Store::Container(c) => c.names().collect(),
            Store::Raw(r) => r.raw.keys().map(String::as_str).collect(),
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        match self {
            Store::Container(c) => c.contains(name),
            Store::Raw(r) => r.raw.contains_key(name),
        }
    }

    pub fn digests(&self) -> IndexMap<String, String> {
        match self {
            Store::Container(c) => c.digests(),
            Store::Raw(r) => r.raw.iter().map(|(n, b)| (n.clone(), hex(&sha256(b)))).collect(),
        }
    }

    pub fn raw(&self, name: &str) -> Result<Cow<'_, [u8]>> {
        match self {
            Store::Container(c) => c.raw(name).map(Cow::Owned),
            Store::Raw(r) => r
                .raw
                .get(name)
                .map(|b| Cow::Borrowed(b.as_slice()))
                .ok_or_else(|| Error::format(format!("missing section: {name}"))),
        }
    }
}
