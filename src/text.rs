use crate::tables::{LOWER_EXCEPTIONS, PRINTABLE, UNASSIGNED, WORD};

fn in_table(table: &[(u32, u32)], c: char) -> bool {
    let cp = c as u32;
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < cp {
                std::cmp::Ordering::Less
            } else if lo > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

pub fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0b}'
            | '\u{0c}'
            | '\r'
            | '\u{1c}'..='\u{20}'
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

pub fn is_word(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    in_table(WORD, c)
}

pub fn is_printable(c: char) -> bool {
    if c.is_ascii() {
        return (' '..='~').contains(&c);
    }
    in_table(PRINTABLE, c)
}

pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

pub fn lower(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    let special = |c: char| in_table(UNASSIGNED, c) || lower_exception(c).is_some();
    if !s.chars().any(special) {
        return s.to_lowercase();
    }
    let swapped: String = s.chars().map(|c| if in_table(UNASSIGNED, c) { '0' } else { c }).collect();
    let lowered = swapped.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut rest = lowered.chars();
    for (c, t) in s.chars().zip(swapped.chars()) {
        let n = if t == '\u{3a3}' { 1 } else { t.to_lowercase().count() };
        if t != c {
            rest.by_ref().take(n).for_each(drop);
            out.push(c);
        } else if let Some(mapped) = lower_exception(c) {
            rest.by_ref().take(n).for_each(drop);
            out.push_str(mapped);
        } else {
            out.extend(rest.by_ref().take(n));
        }
    }
    out
}

fn lower_exception(c: char) -> Option<&'static str> {
    LOWER_EXCEPTIONS.binary_search_by_key(&(c as u32), |&(cp, _)| cp).ok().map(|i| LOWER_EXCEPTIONS[i].1)
}

fn push_hex(out: &mut String, prefix: &str, cp: u32, width: usize) {
    out.push_str(prefix);
    out.push_str(&format!("{cp:0width$x}"));
}

pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        let cp = c as u32;
        if c == quote || c == '\\' {
            out.push('\\');
            out.push(c);
        } else if c == '\t' {
            out.push_str("\\t");
        } else if c == '\n' {
            out.push_str("\\n");
        } else if c == '\r' {
            out.push_str("\\r");
        } else if cp < 0x20 || cp == 0x7f {
            push_hex(&mut out, "\\x", cp, 2);
        } else if cp < 0x7f || is_printable(c) {
            out.push(c);
        } else if cp <= 0xff {
            push_hex(&mut out, "\\x", cp, 2);
        } else if cp <= 0xffff {
            push_hex(&mut out, "\\u", cp, 4);
        } else {
            push_hex(&mut out, "\\U", cp, 8);
        }
    }
    out.push(quote);
    out
}

pub fn repr_list<S: AsRef<str>>(items: &[S]) -> String {
    let parts: Vec<String> = items.iter().map(|s| repr_str(s.as_ref())).collect();
    format!("[{}]", parts.join(", "))
}

pub fn display_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_exact_set() {
        let space: Vec<u32> = vec![
            0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x85, 0xa0, 0x1680, 0x2000, 0x2001,
            0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a, 0x2028, 0x2029, 0x202f,
            0x205f, 0x3000,
        ];
        for cp in 0..=0xffffu32 {
            let Some(c) = char::from_u32(cp) else { continue };
            let s = format!("{c}x{c}");
            let expected = if space.contains(&cp) { "x".to_string() } else { s.clone() };
            assert_eq!(strip(&s), expected, "{cp:x}");
        }
        assert_eq!(strip("\u{feff}a\u{feff}"), "\u{feff}a\u{feff}");
        assert_eq!(strip(" \u{1f600} "), "\u{1f600}");
        assert_eq!(strip(""), "");
    }

    #[test]
    fn lower_cases() {
        assert_eq!(lower("\u{130}"), "i\u{307}");
        assert_eq!(lower("ΟΔΥΣΣΕΥΣ"), "οδυσσευς");
        assert_eq!(lower("cdn:\u{3a3}"), "cdn:\u{3c2}");
        assert_eq!(lower("\u{3a3}"), "\u{3c3}");
        assert_eq!(lower("\u{1e9e}"), "\u{df}");
        assert_eq!(lower("\u{10400}"), "\u{10428}");
        assert_eq!(lower("\u{a7cb}x\u{3a3}"), "\u{a7cb}x\u{3c2}");
        assert_eq!(lower("ÄΣ\u{10d50}"), "äς\u{10d50}");
    }

    #[test]
    fn paths() {
        assert_eq!(display_path("a//b/./c/"), "a/b/c");
        assert_eq!(display_path("/x/"), "/x");
        assert_eq!(display_path("./"), ".");
    }
}
