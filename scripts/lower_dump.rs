// Dumps char::to_lowercase for every code point; input of scripts/gen_tables.py.
//     rustc -O scripts/lower_dump.rs -o /tmp/lower_dump && (cd /tmp && ./lower_dump)

fn main() {
    let mut out = String::new();
    for cp in 0u32..=0x10FFFF {
        let Some(c) = char::from_u32(cp) else { continue };
        if c == '\u{3a3}' { continue; }
        let l: String = c.to_lowercase().collect();
        let codes: Vec<String> = l.chars().map(|x| format!("{:X}", x as u32)).collect();
        out.push_str(&format!("{:X} {}\n", cp, codes.join(",")));
    }
    std::fs::write("rust_lower.txt", out).unwrap();
}
