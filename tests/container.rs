//! Container errors, ported from `tests/test_container_errors.py` and sigdb-ts.

use sigdb::compression::compress_zstd;
use sigdb::format::container::parse_container;
use sigdb::{Error, LoadOptions, Value, build_bytes, load_bytes, load_bytes_with};

fn rules() -> Value {
    Value::from_json(r#"{"nginx": {"headers": {"Server": "nginx"}}, "jquery": {"js": "jquery"}}"#).unwrap()
}

fn data() -> Vec<u8> {
    build_bytes(Some(&rules()), &Default::default()).unwrap()
}

fn table(data: &[u8]) -> Vec<(String, u32, u32, usize)> {
    let header_len = u32::from_be_bytes(data[5..9].try_into().unwrap()) as usize;
    let mut pos = 9 + header_len;
    let count = u16::from_be_bytes([data[pos], data[pos + 1]]);
    pos += 2;
    let mut out = Vec::new();
    for _ in 0..count {
        let n = data[pos] as usize;
        let name = String::from_utf8(data[pos + 1..pos + 1 + n].to_vec()).unwrap();
        pos += 1 + n;
        let raw = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap());
        let body = u32::from_be_bytes(data[pos + 4..pos + 8].try_into().unwrap());
        out.push((name, raw, body, pos));
        pos += 8 + 32;
    }
    out
}

fn format_err(r: Result<impl std::fmt::Debug, Error>, text: &str) {
    match r {
        Err(Error::Format(m)) => assert!(m.contains(text), "{m:?} does not contain {text:?}"),
        other => panic!("expected FormatError containing {text:?}, got {other:?}"),
    }
}

fn head(sections: &[(&[u8], u32, &[u8])]) -> Vec<u8> {
    let mut out = b"SIGT\x03".to_vec();
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(b"{}");
    out.extend_from_slice(&(sections.len() as u16).to_be_bytes());
    for (name, raw, body) in sections {
        out.push(name.len() as u8);
        out.extend_from_slice(name);
        out.extend_from_slice(&raw.to_be_bytes());
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&[0u8; 32]);
    }
    for (_, _, body) in sections {
        out.extend_from_slice(body);
    }
    out
}

#[test]
fn layout() {
    let data = data();
    assert_eq!(&data[..4], b"SIGT");
    assert_eq!(data[4], 3);
    let names: Vec<String> = table(&data).into_iter().map(|t| t.0).collect();
    assert_eq!(names, ["index/main", "automaton/main/headers", "automaton/main/js"]);
}

#[test]
fn header_and_version_errors() {
    let data = data();
    let mut nope = b"NOPE".to_vec();
    nope.extend_from_slice(&[0; 16]);
    format_err(load_bytes(nope), "invalid magic");
    for (v, text) in [
        (1u8, "unsupported sigdb version: 1 (legacy signed format); rebuild the database from rules"),
        (2, "unsupported sigdb version: 2 (single-automaton format); rebuild the database from rules"),
    ] {
        let mut d = b"SIGT".to_vec();
        d.push(v);
        d.extend_from_slice(&[0; 16]);
        format_err(load_bytes(d), text);
    }
    let mut d = b"SIGT\x04".to_vec();
    d.extend_from_slice(&[0; 16]);
    format_err(load_bytes(d), "unsupported sigdb version: 4");
    let mut trailing = data.clone();
    trailing.push(0);
    format_err(load_bytes(trailing), "trailing data after last section");
    format_err(load_bytes(data[..data.len() - 1].to_vec()), "unexpected EOF");
    format_err(load_bytes(data[..7].to_vec()), "unexpected EOF");
}

#[test]
fn hash_verification_is_lazy_and_per_section() {
    let data = data();
    let (name, _, _, pos) = table(&data)[0].clone();
    let mut corrupted = data.clone();
    corrupted[pos + 8] ^= 0x01;
    let db = load_bytes(corrupted.clone()).unwrap();
    assert_eq!(
        db.index("main").unwrap_err(),
        Error::Integrity(format!("corrupted database (hash mismatch in section {name})"))
    );
    let unchecked =
        load_bytes_with(corrupted.clone(), &LoadOptions { verify_hash: false, ..Default::default() })
            .unwrap();
    assert_eq!(unchecked.index("main").unwrap().items[0].key, "nginx");
    assert_eq!(load_bytes(corrupted).unwrap().metadata["format"], Value::from("SIGDB"));
}

#[test]
fn zstd_frame_size_checks() {
    let data = data();
    let (_, raw, _, pos) = table(&data)[0].clone();
    let mut wrong = data.clone();
    wrong[pos..pos + 4].copy_from_slice(&(raw + 1).to_be_bytes());
    format_err(
        load_bytes(wrong).unwrap().index("main"),
        "section index/main: zstd frame size does not match section size",
    );

    let bomb = compress_zstd(&vec![b'a'; 10_000_000], 1).unwrap();
    let file = head(&[(b"index/main", 100, &bomb)]);
    let c = parse_container(file.clone(), true, 512 * 1024 * 1024).unwrap();
    format_err(c.raw("index/main"), "zstd frame size");
    let c = parse_container(file, true, 10).unwrap();
    format_err(c.raw("index/main"), "section index/main exceeds max_section_size");
}

#[test]
fn invalid_section_contents() {
    let body = compress_zstd(&[0xff, 0xff], 1).unwrap();
    let garbage = head(&[(b"index/main", 2, &body)]);
    let db = load_bytes_with(garbage, &LoadOptions { verify_hash: false, ..Default::default() }).unwrap();
    format_err(db.index("main"), "invalid index main json");

    format_err(load_bytes(head(&[])), "database has no indexes");
    let not_zstd = head(&[(b"index/main", 2, b"abc")]);
    format_err(load_bytes(not_zstd).unwrap().index("main"), "section index/main: invalid zstd frame");
}

#[test]
fn section_table_errors() {
    format_err(load_bytes(head(&[(&[0xff], 0, b"")])), "invalid section name");
    format_err(load_bytes(head(&[(b"", 0, b"")])), "invalid or duplicate section name: ''");
    format_err(
        load_bytes(head(&[(b"json/a", 0, b""), (b"json/a", 0, b"")])),
        "invalid or duplicate section name: 'json/a'",
    );
    format_err(load_bytes(head(&[(b"weird/x", 0, b"")])), "unknown section: weird/x");
    format_err(load_bytes(head(&[(b"index/", 0, b"")])), "unknown section: index/");
    let mut big = b"SIGT\x03".to_vec();
    big.extend_from_slice(&65_537u32.to_be_bytes());
    format_err(load_bytes(big), "HEADER_DATA too large");
    let mut arr = b"SIGT\x03".to_vec();
    arr.extend_from_slice(&2u32.to_be_bytes());
    arr.extend_from_slice(b"[]");
    format_err(load_bytes(arr), "HEADER_DATA must be an object");
    let mut bad = b"SIGT\x03".to_vec();
    bad.extend_from_slice(&2u32.to_be_bytes());
    bad.extend_from_slice(b"{x");
    format_err(load_bytes(bad), "invalid HEADER_DATA json");
}

#[test]
fn varint_errors() {
    use sigdb::utils::varint::{decode_varint, encode_varint};
    for v in [0u64, 1, 2, 10, 127, 128, 255, 300, 16_384, 1 << 32, (1 << 63) - 1] {
        let enc = encode_varint(v);
        let dec = decode_varint(&enc, 0, 10).unwrap();
        assert_eq!((dec.value, dec.offset), (v, enc.len()));
    }
    assert!(decode_varint(&[], 0, 10).unwrap_err().message().contains("truncated varint"));
    assert!(decode_varint(&[0x80], 0, 10).unwrap_err().message().contains("truncated varint"));
    assert!(decode_varint(&[0x80; 10], 0, 10).unwrap_err().message().contains("varint too long"));
    let enc = encode_varint(300);
    assert_ne!(decode_varint(&enc, 1, 10).unwrap().value, 300);
}
