//! JSON reading and writing that matches Python's `json` module byte for byte:
//! `json.loads` (with `NaN`/`Infinity`, encoding detection for bytes, last duplicate key wins in
//! the position of the first) and `json.dumps(ensure_ascii=False, separators=(",", ":"),
//! allow_nan=False)` (with `float.__repr__` for floats).

use crate::value::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError(pub String);

type Result<T> = std::result::Result<T, JsonError>;

const MAX_DEPTH: usize = 1000;

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn fail<T>(&self, msg: &str) -> Result<T> {
        Err(JsonError(format!("{msg}: char {}", self.pos)))
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.pos += 1;
        }
    }

    fn starts_with(&self, lit: &[u8]) -> bool {
        self.s[self.pos..].starts_with(lit)
    }

    fn value(&mut self) -> Result<Value> {
        match self.peek() {
            Some(b'"') => {
                self.pos += 1;
                Ok(Value::String(self.string()?))
            }
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'n') => self.literal(b"null", Value::Null),
            Some(b't') => self.literal(b"true", Value::Bool(true)),
            Some(b'f') => self.literal(b"false", Value::Bool(false)),
            Some(b'N') => self.literal(b"NaN", Value::Float(f64::NAN)),
            Some(b'I') => self.literal(b"Infinity", Value::Float(f64::INFINITY)),
            Some(b'-') if self.starts_with(b"-Infinity") => {
                self.pos += 9;
                Ok(Value::Float(f64::NEG_INFINITY))
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => self.fail("Expecting value"),
        }
    }

    fn literal(&mut self, lit: &[u8], v: Value) -> Result<Value> {
        if !self.starts_with(lit) {
            return self.fail("Expecting value");
        }
        self.pos += lit.len();
        Ok(v)
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while let Some(b'0'..=b'9') = self.peek() {
            self.pos += 1;
        }
        self.pos - start
    }

    fn number(&mut self) -> Result<Value> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => {
                self.pos = start;
                return self.fail("Expecting value");
            }
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') && matches!(self.s.get(self.pos + 1), Some(b'0'..=b'9')) {
            self.pos += 1;
            self.digits();
            is_float = true;
        }
        if let Some(b'e' | b'E') = self.peek() {
            let save = self.pos;
            self.pos += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.pos += 1;
            }
            if self.digits() == 0 {
                self.pos = save;
            } else {
                is_float = true;
            }
        }
        let text = &self.text[start..self.pos];
        if is_float {
            let f: f64 = text.parse().map_err(|_| JsonError(format!("bad number {text}")))?;
            return Ok(Value::Float(f));
        }
        Ok(int_value(text))
    }

    fn hex4(&mut self, at: usize) -> Result<u32> {
        let h = self.s.get(at..at + 4).filter(|h| h.iter().all(u8::is_ascii_hexdigit));
        match h {
            Some(h) => Ok(u32::from_str_radix(std::str::from_utf8(h).unwrap_or("0"), 16).unwrap_or(0)),
            None => {
                self.pos = at;
                self.fail("Invalid \\uXXXX escape")
            }
        }
    }

    fn string(&mut self) -> Result<String> {
        let begin = self.pos;
        // Fast path: no escapes before the closing quote.
        let rest = &self.s[begin..];
        if let Some(i) = rest.iter().position(|&b| b == b'"' || b == b'\\' || b < 0x20) {
            if rest[i] == b'"' {
                self.pos = begin + i + 1;
                return Ok(self.text[begin..begin + i].to_string());
            }
        }
        let mut out = String::new();
        let mut chunk = self.pos;
        loop {
            let Some(c) = self.peek() else {
                self.pos = begin - 1;
                return self.fail("Unterminated string starting at");
            };
            if c == b'"' {
                out.push_str(&self.text[chunk..self.pos]);
                self.pos += 1;
                return Ok(out);
            }
            if c < 0x20 {
                return self.fail("Invalid control character at");
            }
            if c != b'\\' {
                self.pos += 1;
                continue;
            }
            out.push_str(&self.text[chunk..self.pos]);
            let esc = self.s.get(self.pos + 1).copied();
            let ch = match esc {
                Some(b'"') => '"',
                Some(b'\\') => '\\',
                Some(b'/') => '/',
                Some(b'b') => '\u{8}',
                Some(b'f') => '\u{c}',
                Some(b'n') => '\n',
                Some(b'r') => '\r',
                Some(b't') => '\t',
                Some(b'u') => {
                    let mut cp = self.hex4(self.pos + 2)?;
                    self.pos += 6;
                    if (0xd800..=0xdbff).contains(&cp) && self.starts_with(b"\\u") {
                        let lo = self.hex4(self.pos + 2)?;
                        if (0xdc00..=0xdfff).contains(&lo) {
                            cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                            self.pos += 6;
                        }
                    }
                    match char::from_u32(cp) {
                        Some(ch) => out.push(ch),
                        None => return self.fail("Lone surrogate in string"),
                    }
                    chunk = self.pos;
                    continue;
                }
                _ => return self.fail("Invalid \\escape"),
            };
            out.push(ch);
            self.pos += 2;
            chunk = self.pos;
        }
    }

    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return self.fail("Nesting too deep");
        }
        Ok(())
    }

    fn object(&mut self) -> Result<Value> {
        self.enter()?;
        self.pos += 1;
        let mut map = Map::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Value::Object(map));
        }
        loop {
            if self.peek() != Some(b'"') {
                return self.fail("Expecting property name enclosed in double quotes");
            }
            self.pos += 1;
            let key = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return self.fail("Expecting ':' delimiter");
            }
            self.pos += 1;
            self.ws();
            let v = self.value()?;
            map.insert(key, v);
            self.ws();
            match self.peek() {
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                Some(b',') => self.pos += 1,
                _ => return self.fail("Expecting ',' delimiter"),
            }
            self.ws();
        }
        self.depth -= 1;
        Ok(Value::Object(map))
    }

    fn array(&mut self) -> Result<Value> {
        self.enter()?;
        self.pos += 1;
        let mut out = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Value::Array(out));
        }
        loop {
            self.ws();
            out.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                Some(b',') => self.pos += 1,
                _ => return self.fail("Expecting ',' delimiter"),
            }
        }
        self.depth -= 1;
        Ok(Value::Array(out))
    }
}

/// Step-by-step access to the parser for payloads of a known shape. Every method answers
/// `None`/`false` when the text does not continue the expected way; the caller then falls back to
/// [`parse`], which decides what the text means (and how it is wrong).
pub(crate) struct Scanner<'a>(Parser<'a>);

impl<'a> Scanner<'a> {
    pub fn new(text: &'a str) -> Self {
        Scanner(Parser { s: text.as_bytes(), text, pos: 0, depth: 0 })
    }

    pub fn eat(&mut self, b: u8) -> bool {
        self.0.ws();
        if self.0.peek() == Some(b) {
            self.0.pos += 1;
            true
        } else {
            false
        }
    }

    pub fn string(&mut self) -> Option<String> {
        self.0.ws();
        if self.0.peek() != Some(b'"') {
            return None;
        }
        self.0.pos += 1;
        self.0.string().ok()
    }

    /// A value nested `depth` containers deep, so that the nesting limit counts from the top of
    /// the document as it does in [`parse`].
    pub fn value_at(&mut self, depth: usize) -> Option<Value> {
        self.0.ws();
        self.0.depth = depth;
        let v = self.0.value().ok();
        self.0.depth = 0;
        v
    }

    /// A non-negative integer literal (not a float, no leading zeros).
    pub fn uint(&mut self) -> Option<u64> {
        self.0.ws();
        let start = self.0.pos;
        let n = self.0.digits();
        if n == 0 || (n > 1 && self.0.s[start] == b'0') {
            return None;
        }
        if let Some(b'.' | b'e' | b'E') = self.0.peek() {
            return None;
        }
        self.0.text[start..self.0.pos].parse().ok()
    }

    pub fn end(&mut self) -> bool {
        self.0.ws();
        self.0.pos == self.0.s.len()
    }
}

fn int_value(text: &str) -> Value {
    match text.parse::<i64>() {
        Ok(i) => Value::Int(i),
        Err(_) => {
            let (neg, digits) = match text.strip_prefix('-') {
                Some(d) => (true, d),
                None => (false, text),
            };
            let digits = digits.trim_start_matches('0');
            if digits.is_empty() {
                Value::Int(0)
            } else if neg {
                Value::BigInt(format!("-{digits}"))
            } else {
                Value::BigInt(digits.to_string())
            }
        }
    }
}

/// `json.loads(text)`.
pub fn parse(text: &str) -> Result<Value> {
    let mut p = Parser { s: text.as_bytes(), text, pos: 0, depth: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.pos != text.len() {
        return p.fail("Extra data");
    }
    Ok(v)
}

/// `json.loads(data)` for `bytes`: detects UTF-8/16/32 like `json.detect_encoding`.
pub fn parse_bytes(data: &[u8]) -> Result<Value> {
    let text = decode_bytes(data)?;
    parse(&text)
}

fn detect_encoding(b: &[u8]) -> &'static str {
    if b.starts_with(&[0x00, 0x00, 0xfe, 0xff]) || b.starts_with(&[0xff, 0xfe, 0x00, 0x00]) {
        return "utf-32";
    }
    if b.starts_with(&[0xfe, 0xff]) || b.starts_with(&[0xff, 0xfe]) {
        return "utf-16";
    }
    if b.starts_with(&[0xef, 0xbb, 0xbf]) {
        return "utf-8-sig";
    }
    if b.len() >= 4 {
        if b[0] == 0 {
            return if b[1] != 0 { "utf-16-be" } else { "utf-32-be" };
        }
        if b[1] == 0 {
            return if b[2] != 0 || b[3] != 0 { "utf-16-le" } else { "utf-32-le" };
        }
    } else if b.len() == 2 {
        if b[0] == 0 {
            return "utf-16-be";
        }
        if b[1] == 0 {
            return "utf-16-le";
        }
    }
    "utf-8"
}

fn decode_bytes(b: &[u8]) -> Result<std::borrow::Cow<'_, str>> {
    let bad = |enc: &str| JsonError(format!("cannot decode JSON bytes as {enc}"));
    let enc = detect_encoding(b);
    match enc {
        "utf-8" => std::str::from_utf8(b).map(Into::into).map_err(|_| bad(enc)),
        "utf-8-sig" => std::str::from_utf8(&b[3..]).map(Into::into).map_err(|_| bad(enc)),
        "utf-16" => utf16(&b[2..], b[0] == 0xff).map(Into::into).ok_or_else(|| bad(enc)),
        "utf-16-le" => utf16(b, true).map(Into::into).ok_or_else(|| bad(enc)),
        "utf-16-be" => utf16(b, false).map(Into::into).ok_or_else(|| bad(enc)),
        "utf-32" => utf32(&b[4..], b[0] == 0xff).map(Into::into).ok_or_else(|| bad(enc)),
        "utf-32-le" => utf32(b, true).map(Into::into).ok_or_else(|| bad(enc)),
        _ => utf32(b, false).map(Into::into).ok_or_else(|| bad(enc)),
    }
}

fn utf16(b: &[u8], le: bool) -> Option<String> {
    if b.len() % 2 != 0 {
        return None;
    }
    let units = b
        .chunks_exact(2)
        .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) });
    char::decode_utf16(units).collect::<std::result::Result<String, _>>().ok()
}

fn utf32(b: &[u8], le: bool) -> Option<String> {
    if b.len() % 4 != 0 {
        return None;
    }
    b.chunks_exact(4)
        .map(|c| {
            let a = [c[0], c[1], c[2], c[3]];
            char::from_u32(if le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
        })
        .collect()
}

fn escape_into(out: &mut String, s: &str) {
    out.push('"');
    let bytes = s.as_bytes();
    let mut start = 0;
    for (i, &c) in bytes.iter().enumerate() {
        let esc: &str = match c {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\n' => "\\n",
            b'\r' => "\\r",
            b'\t' => "\\t",
            0x08 => "\\b",
            0x0c => "\\f",
            0x00..=0x1f => "",
            _ => continue,
        };
        out.push_str(&s[start..i]);
        if esc.is_empty() {
            out.push_str(&format!("\\u{c:04x}"));
        } else {
            out.push_str(esc);
        }
        start = i + 1;
    }
    out.push_str(&s[start..]);
    out.push('"');
}

/// `float.__repr__` for a finite float.
pub fn float_repr(x: f64) -> String {
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if negative { "-" } else { "" };
    if !(-4..16).contains(&exp) {
        let m = if digits.len() > 1 { format!("{}.{}", &digits[..1], &digits[1..]) } else { digits };
        let e = if exp < 0 { format!("-{:02}", -exp) } else { format!("+{exp:02}") };
        return format!("{sign}{m}e{e}");
    }
    if exp < 0 {
        return format!("{sign}0.{}{digits}", "0".repeat((-exp - 1) as usize));
    }
    let int_len = (exp + 1) as usize;
    if digits.len() <= int_len {
        return format!("{sign}{digits}{}.0", "0".repeat(int_len - digits.len()));
    }
    format!("{sign}{}.{}", &digits[..int_len], &digits[int_len..])
}

/// `repr()` of a Python float, `inf` and `nan` included.
pub fn float_py_repr(x: f64) -> String {
    if x.is_nan() {
        "nan".into()
    } else if x.is_infinite() {
        if x > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        float_repr(x)
    }
}

fn dump_into(out: &mut String, v: &Value) -> Result<()> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::BigInt(s) => out.push_str(s),
        Value::Float(f) => {
            if !f.is_finite() {
                return Err(JsonError(format!(
                    "Out of range float values are not JSON compliant: {}",
                    float_py_repr(*f)
                )));
            }
            out.push_str(&float_repr(*f));
        }
        Value::String(s) => escape_into(out, s),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                dump_into(out, x)?;
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                escape_into(out, k);
                out.push(':');
                dump_into(out, x)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// `json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False)`.
pub fn dump(v: &Value) -> Result<String> {
    let mut out = String::new();
    dump_into(&mut out, v)?;
    Ok(out)
}

/// Writer used for large payloads built without an intermediate [`Value`].
pub(crate) struct Writer {
    pub out: String,
}

impl Writer {
    pub fn new() -> Self {
        Writer { out: String::new() }
    }

    pub fn raw(&mut self, s: &str) {
        self.out.push_str(s);
    }

    pub fn string(&mut self, s: &str) {
        escape_into(&mut self.out, s);
    }

    pub fn value(&mut self, v: &Value) -> Result<()> {
        dump_into(&mut self.out, v)
    }

    pub fn uint(&mut self, n: usize) {
        use std::fmt::Write;
        let _ = write!(self.out, "{n}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_and_duplicates() {
        let v = parse(r#"{"10":1,"2":2,"a":3,"2":4}"#).unwrap();
        let m = v.as_object().unwrap();
        let pairs: Vec<(&str, i64)> = m.iter().map(|(k, v)| (k.as_str(), v.as_i64().unwrap())).collect();
        assert_eq!(pairs, vec![("10", 1), ("2", 4), ("a", 3)]);
    }

    #[test]
    fn literals_and_numbers() {
        let v = parse("[NaN,Infinity,-Infinity,-0,1e2,0.5,12345678901234567890123,-0.0]").unwrap();
        let a = v.as_array().unwrap();
        assert!(matches!(a[0], Value::Float(f) if f.is_nan()));
        assert_eq!(a[1], Value::Float(f64::INFINITY));
        assert_eq!(a[2], Value::Float(f64::NEG_INFINITY));
        assert_eq!(a[3], Value::Int(0));
        assert_eq!(a[4], Value::Float(100.0));
        assert_eq!(a[6], Value::BigInt("12345678901234567890123".into()));
        assert_eq!(dump(&a[7]).unwrap(), "-0.0");
    }

    #[test]
    fn strings() {
        assert_eq!(parse(r#""\ud83d\ude00""#).unwrap(), Value::String("\u{1f600}".into()));
        assert_eq!(parse(r#""\/\b\f\n\r\t\"\\""#).unwrap(), Value::String("/\u{8}\u{c}\n\r\t\"\\".into()));
        assert!(parse(r#""\ud800x""#).is_err());
    }

    #[test]
    fn invalid() {
        for text in [
            "",
            " ",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "{'a':1}",
            "\"\u{1}\"",
            "\"\\x\"",
            "\"\\u12G4\"",
            "01",
            "1.",
            "tru",
            "[] []",
            "\u{feff}[]",
            "+1",
        ] {
            assert!(parse(text).is_err(), "{text:?}");
        }
        let deep = format!("{}{}", "[".repeat(2000), "]".repeat(2000));
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn encodings() {
        let doc = "{\"k\":\"\u{e9}\"}";
        let expected = parse(doc).unwrap();
        assert_eq!(parse_bytes(doc.as_bytes()).unwrap(), expected);
        let mut bom = vec![0xef, 0xbb, 0xbf];
        bom.extend_from_slice(doc.as_bytes());
        assert_eq!(parse_bytes(&bom).unwrap(), expected);
        let le: Vec<u8> = doc.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(parse_bytes(&le).unwrap(), expected);
        let be: Vec<u8> = doc.encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(parse_bytes(&be).unwrap(), expected);
        let mut le_bom = vec![0xff, 0xfe];
        le_bom.extend_from_slice(&le);
        assert_eq!(parse_bytes(&le_bom).unwrap(), expected);
        let u32le: Vec<u8> = doc.chars().flat_map(|c| (c as u32).to_le_bytes()).collect();
        assert_eq!(parse_bytes(&u32le).unwrap(), expected);
        assert!(parse_bytes(&[0x22, 0xff, 0x22]).is_err());
    }

    #[test]
    fn floats() {
        let cases = [
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (123456789012345678.0, "1.2345678901234568e+17"),
            (1e22, "1e+22"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (0.1, "0.1"),
            (100.0, "100.0"),
            (1e15, "1000000000000000.0"),
            (9999999999999998.0, "9999999999999998.0"),
            (-2.5, "-2.5"),
        ];
        for (x, s) in cases {
            assert_eq!(float_repr(x), s);
        }
        assert_eq!(
            dump(&Value::Float(f64::NAN)).unwrap_err().0,
            "Out of range float values are not JSON compliant: nan"
        );
    }

    #[test]
    fn escapes() {
        let s = Value::String("a\"\\\n\r\t\u{8}\u{c}\u{1}\u{1f}\u{7f}é\u{2028}".into());
        assert_eq!(dump(&s).unwrap(), "\"a\\\"\\\\\\n\\r\\t\\b\\f\\u0001\\u001f\u{7f}é\u{2028}\"");
    }
}
