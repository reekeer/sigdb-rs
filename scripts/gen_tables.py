from __future__ import annotations

import sys
import unicodedata


def ranges(pred) -> list[tuple[int, int]]:
    out: list[tuple[int, int]] = []
    start = None
    for cp in range(0x110000):
        if 0xD800 <= cp <= 0xDFFF:
            ok = False
        else:
            ok = pred(chr(cp))
        if ok and start is None:
            start = cp
        elif not ok and start is not None:
            out.append((start, cp - 1))
            start = None
    if start is not None:
        out.append((start, 0x10FFFF))
    return out


def emit(name: str, rs: list[tuple[int, int]]) -> None:
    print(f"pub(crate) static {name}: &[(u32, u32)] = &[")
    for a, b in rs:
        print(f"    (0x{a:04X}, 0x{b:04X}),")
    print("];")
    print()


emit("PRINTABLE", ranges(lambda c: c.isprintable()))
emit("WORD", ranges(lambda c: c.isalnum() or c == "_"))
emit("UNASSIGNED", ranges(lambda c: unicodedata.category(c) == "Cn"))

exceptions: list[tuple[int, str]] = []
if len(sys.argv) > 1:
    for line in open(sys.argv[1], encoding="ascii"):
        cp_hex, codes = line.split()
        cp = int(cp_hex, 16)
        rust = "".join(chr(int(x, 16)) for x in codes.split(","))
        if chr(cp).lower() != rust:
            exceptions.append((cp, chr(cp).lower()))
print("pub(crate) static LOWER_EXCEPTIONS: &[(u32, &str)] = &[")
for cp, low in exceptions:
    text = "".join(f"\\u{{{ord(c):x}}}" for c in low)
    print(f'    (0x{cp:04X}, "{text}"),')
print("];")
