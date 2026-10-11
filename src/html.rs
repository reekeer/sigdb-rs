//! Tag and attribute heads of an HTML document, matched the way the Python package's regular
//! expressions match them:
//!
//! ```text
//! tag:  <\s*([a-zA-Z][\w:-]*)\b([^<>]*)>                          (re.IGNORECASE)
//! attr: ([a-zA-Z_:][\w:.-]*)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?
//! ```
//!
//! `\w`, `\s` and `\b` follow Python's Unicode definitions (see [`crate::text`]).

use std::collections::HashSet;

use crate::text::{is_space, is_word, strip};

fn tag_first(c: char) -> bool {
    // [a-zA-Z] under re.IGNORECASE also matches the characters that case-fold into it.
    c.is_ascii_alphabetic() || matches!(c, '\u{130}' | '\u{131}' | '\u{17f}' | '\u{212a}')
}

fn tag_char(c: char) -> bool {
    is_word(c) || c == ':' || c == '-'
}

fn attr_first(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == ':'
}

fn attr_char(c: char) -> bool {
    is_word(c) || c == ':' || c == '.' || c == '-'
}

fn unquoted_char(c: char) -> bool {
    !is_space(c) && !matches!(c, '"' | '\'' | '=' | '<' | '>' | '`')
}

type Attrs = Vec<(String, Option<String>)>;

/// `(tag, [(attr, value)])` for every tag the tag expression finds.
fn tags(html: &[char]) -> Vec<(String, Attrs)> {
    let mut out = Vec::new();
    let n = html.len();
    let mut i = 0;
    while i < n {
        if html[i] != '<' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < n && is_space(html[j]) {
            j += 1;
        }
        if j >= n || !tag_first(html[j]) {
            i += 1;
            continue;
        }
        let mut m = j + 1;
        while m < n && tag_char(html[m]) {
            m += 1;
        }
        // \b after the name: back off to just past its last word character. html[j] is one,
        // so the name never shrinks below a single character.
        let mut p = m;
        while p > j + 1 && !is_word(html[p - 1]) {
            p -= 1;
        }
        let mut q = m;
        while q < n && html[q] != '<' && html[q] != '>' {
            q += 1;
        }
        if q >= n || html[q] != '>' {
            i += 1;
            continue;
        }
        let tag: String = html[j..p].iter().collect();
        out.push((tag, attrs(&html[p..q])));
        i = q + 1;
    }
    out
}

fn attrs(s: &[char]) -> Attrs {
    let mut out = Vec::new();
    let n = s.len();
    let mut i = 0;
    while i < n {
        if !attr_first(s[i]) {
            i += 1;
            continue;
        }
        let mut end = i + 1;
        while end < n && attr_char(s[end]) {
            end += 1;
        }
        let name: String = s[i..end].iter().collect();
        let (value, next) = attr_value(s, end);
        let value = value.filter(|v| !v.is_empty()).map(|v| strip(&v).to_string());
        out.push((name, value));
        i = next;
    }
    out
}

/// The optional `\s*=\s*(...)` part at `at`: the raw value and where the match ends.
fn attr_value(s: &[char], at: usize) -> (Option<String>, usize) {
    let n = s.len();
    let mut k = at;
    while k < n && is_space(s[k]) {
        k += 1;
    }
    if k >= n || s[k] != '=' {
        return (None, at);
    }
    k += 1;
    while k < n && is_space(s[k]) {
        k += 1;
    }
    if k >= n {
        return (None, at);
    }
    let quote = s[k];
    if quote == '"' || quote == '\'' {
        return match s[k + 1..].iter().position(|&c| c == quote) {
            Some(len) => (Some(s[k + 1..k + 1 + len].iter().collect()), k + 2 + len),
            None => (None, at),
        };
    }
    let start = k;
    while k < n && unquoted_char(s[k]) {
        k += 1;
    }
    if k == start {
        return (None, at);
    }
    (Some(s[start..k].iter().collect()), k)
}

/// Every head of `html` in document order without repeats: `tag:T`, `tag:T:attr:A`,
/// `tag:T:attr:A:value:V`, `attr:A`, `attr:A:value:V`.
pub fn html_heads(html: &str) -> Vec<String> {
    let chars: Vec<char> = html.chars().collect();
    let mut heads = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |v: String| {
        if seen.insert(v.clone()) {
            heads.push(v);
        }
    };
    for (tag, attrs) in tags(&chars) {
        add(format!("tag:{tag}"));
        for (name, value) in attrs {
            add(format!("tag:{tag}:attr:{name}"));
            if let Some(v) = &value {
                add(format!("tag:{tag}:attr:{name}:value:{v}"));
            }
            add(format!("attr:{name}"));
            if let Some(v) = &value {
                add(format!("attr:{name}:value:{v}"));
            }
        }
    }
    heads
}
