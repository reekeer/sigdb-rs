from __future__ import annotations

import json
import random
import shutil
import sys
import unicodedata
from pathlib import Path
from typing import Any

from sigdb import Database, Error, build_bytes, load_bytes
from sigdb.internal.groups import BUILTIN_GROUPS
from sigdb.types import Hit, MatchResult

WORDS = [
    "nginx", "Apache", "react", "Vue", "jquery", "wp-content", "Server", "X-Powered-By", "generator",
    "WordPress 6.4", "cdn.jsdelivr.net", "/wp-admin", "next", "__NEXT_DATA__", "P:useState",
    "K:subscribe", "S:react.element", "Minified React error #", "data-reactroot", "ng-version",
    "İstanbul", "ISTANBUL", "Straße", "STRASSE", "ΣΑΣ", "ΟΔΥΣΣΕΥΣ", "σς", "ǅungla", "ﬀ", "K", "ſ",
    "é", "café", "naïve", "🚀 caddy", "日本語", "عربي", "​zw", "Ᲊx", "\U00010d50y",
    "ꟋZ", "Σ\U00010d50", "a:b", ":lead", "trail:", "  pad  ", "　wide　", "\x1cfs\x1f",
    "\x85nel", "\xa0nb", "﻿bom", "tab\there", "q\"uote", "back\\slash", "<tag>", "a=b", "`tick`",
    "x", "X", "ab", "abc", "ABC", "Ab", "a", "b", "-", "_", ".", "0", "10", "2",
]
HEADER_NAMES = ["Server", "X-Powered-By", "Set-Cookie", "X-Generator", "Via", "CF-Ray", "X-Ünï", " Pad ", "a:b"]
HTML_TAGS = ["div", "META", "script", "svelte-app", "my-app", "x:y", "a-", "İmg", "ſpan", "K-el", "app-root", "日本"]
HTML_ATTRS = ["id", "class", "data-reactroot", "ng-version", "NAME", "content", "_x", ":on", "data.v", "x-"]
FLOATS = [0.1, 1e16, 1.5e16, -0.0, 0.0, 1e-7, 1e-5, 0.0001, 123.0, 5e-324, 1.7976931348623157e308, 2.5, -1e22, 100.0]
INTS = [0, 1, -1, 7, 2**31, 2**53 + 1, 2**63 - 1, 2**63, -(2**63) - 1, 10**30, -(10**25)]


def rword(r: random.Random) -> str:
    w = r.choice(WORDS)
    roll = r.random()
    if roll < 0.15:
        w = w.upper()
    elif roll < 0.3:
        w = w.lower()
    elif roll < 0.4:
        w = r.choice([" ", "\t", "　", "\x1c", "﻿", ""]) + w + r.choice([" ", "\n", "\x85", ""])
    elif roll < 0.5:
        w = w + r.choice(WORDS)
    return w


def rjson(r: random.Random, depth: int = 0) -> Any:
    roll = r.random()
    if depth > 2 or roll < 0.35:
        return r.choice([None, True, False, r.choice(INTS), r.choice(FLOATS), rword(r), r.randint(-50, 50)])
    if roll < 0.65:
        return [rjson(r, depth + 1) for _ in range(r.randint(0, 4))]
    return {rword(r): rjson(r, depth + 1) for _ in range(r.randint(0, 4))}


def rgroups(r: random.Random) -> dict[str, Any]:
    groups: dict[str, Any] = {}
    for _ in range(r.randint(0, 4)):
        name = r.choice(["features", "strings", "links", "names", "m", "f.v", "x-1", "_g"] + list(BUILTIN_GROUPS[:4]))
        cfg: dict[str, Any] = {}
        if r.random() < 0.5:
            cfg["match"] = r.choice(["prefix", "contains", "exact"])
        if r.random() < 0.3:
            cfg["kind"] = r.choice(["list", "map"])
        if r.random() < 0.4:
            cfg["ignore_case"] = r.random() < 0.5
        if r.random() < 0.4:
            cfg["trim"] = r.random() < 0.5
        if r.random() < 0.03:
            cfg[r.choice(["match", "kind", "ignore_case", "case"])] = r.choice(["regex", 1, "yes", None])
        groups[name] = cfg
    if r.random() < 0.01:
        groups[r.choice(["data", "bad/name", "", "a" * 129])] = {}
    return groups


def rvalue(r: random.Random, group: str, kind: str) -> Any:
    if r.random() < 0.02:
        return r.choice([5, None, {"x": 1}, [1], ""])
    if kind == "map":
        return {r.choice(HEADER_NAMES): rword(r) for _ in range(r.randint(1, 3))}
    if group == "html" and r.random() < 0.5:
        spec: dict[str, str] = {}
        if r.random() < 0.7:
            spec["tag"] = r.choice(HTML_TAGS)
        if r.random() < 0.7 or not spec:
            spec["attr"] = r.choice(HTML_ATTRS)
            if r.random() < 0.5:
                spec["value"] = rword(r)
        return spec if r.random() < 0.5 else [spec, rword(r)]
    if r.random() < 0.4:
        return rword(r)
    return [rword(r) for _ in range(r.randint(0, 4))]


def kinds_of(groups: dict[str, Any]) -> dict[str, str]:
    kinds = {g: ("map" if g in ("headers", "meta") else "list") for g in BUILTIN_GROUPS}
    for name, cfg in groups.items():
        if isinstance(cfg, dict):
            kinds[name] = cfg.get("kind", kinds.get(name, "list")) if cfg.get("kind") in ("list", "map") else kinds.get(name, "list")
    return kinds


def rrules(r: random.Random, groups: dict[str, Any]) -> Any:
    kinds = kinds_of(groups)
    names = list(kinds)
    rules: dict[str, Any] = {}
    for _ in range(r.randint(0, 10)):
        key = rword(r) + r.choice(["", "@1.0.0/index.js", "/dist/a.js", str(r.randint(0, 99))])
        rule: dict[str, Any] = {}
        if r.random() < 0.6:
            rule["data"] = rjson(r)
        if r.random() < 0.002:
            rule["data"] = float("nan")
        for _ in range(r.randint(0, 4)):
            g = r.choice(names) if r.random() > 0.003 else "bogus"
            rule[g] = rvalue(r, g, kinds.get(g, "list"))
        rules[key or "k"] = rule
    if r.random() < 0.1:
        keys = list(rules)
        return [{k: rules[k] for k in keys[: len(keys) // 2]}, {k: rules[k] for k in keys[len(keys) // 2 :]}]
    return rules


def rhtml(r: random.Random) -> str:
    parts = []
    for _ in range(r.randint(1, 5)):
        tag = r.choice(HTML_TAGS)
        attrs = []
        for _ in range(r.randint(0, 3)):
            a = r.choice(HTML_ATTRS)
            v = rword(r).replace("<", "").replace(">", "")
            style = r.randint(0, 4)
            if style == 0:
                attrs.append(a)
            elif style == 1:
                attrs.append(f'{a}="{v}"')
            elif style == 2:
                attrs.append(f"{a}='{v}'")
            elif style == 3:
                attrs.append(f"{a} = {v.replace(' ', '')}")
            else:
                attrs.append(f'{a}=""')
        sp = r.choice([" ", "\n", "\t", "  ", "　"])
        parts.append(f"<{r.choice(['', ' '])}{tag}{sp if attrs else ''}{sp.join(attrs)}{r.choice(['', '/', ' '])}>{rword(r)}</{tag}>")
    return r.choice(["", "<!doctype html>", "text "]).join(["", ""]) + "".join(parts)


def result(m: MatchResult) -> dict[str, Any]:
    return {
        "result": m.result,
        "item_id": m.item_id,
        "key": m.item.key if m.item is not None else None,
        "head": m.head,
        "pattern_id": m.pattern_id,
    }


def hits(hs: list[Hit]) -> list[list[Any]]:
    return [[h.item_id, h.item.key, h.hits, list(h.pattern_ids)] for h in hs]


def run(db: Database, v: dict[str, Any]) -> Any:
    call = v["call"]
    try:
        index = db.index(v.get("index", "main"))
        if call == "match":
            out = result(index.match(v["head"]))
            out["all"] = hits(index.match_all("headers", v["head"]))
            return out
        if call == "match_group":
            g, value, name = v["group"], v["value"], v.get("name")
            out = result(index.match_group(g, value, name=name))
            out["all"] = hits(index.match_all(g, {name: value} if name else [value]))
            return out
        if call == "match_html":
            return result(index.match_html(v["html"]))
        if call == "match_search":
            return result(index.match_search(v["search"]))
        if call == "match_all":
            return hits(index.match_all(v["group"], v["values"]))
        if call == "match_tokens":
            return [[i, c] for i, c in index.match_tokens(v["group"], v["tokens"]).items()]
        if call == "scan":
            return [[o.pattern_id, o.start, o.end, index.pattern(o.pattern_id).text] for o in index.scan(v["text"], v["group"])]
        if call == "item":
            item = index.item(v["key"])
            return None if item is None else [index.item_id(v["key"]), item.data]
        if call == "items_with_prefix":
            return [i.key for i in index.items_with_prefix(v["prefix"])]
        if call == "pattern_count":
            return index.pattern_count(v["item_id"], v.get("group"))
        if call == "section":
            value = db.section(v["name"])
            return {"hex": value.hex()} if isinstance(value, bytes) else value
    except Error as e:
        return {"error": type(e).__name__, "message": str(e)}
    raise AssertionError(call)


def rvectors(r: random.Random, db: Database, build: dict[str, Any]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for index_name in db.index_names:
        index = db.index(index_name)
        texts = [index.pattern(i).text for i in range(index.pattern_total)] or ["x"]
        groups = list(index.groups) + ["nope"]
        keys = [i.key for i in index.items] or ["k"]

        def probe() -> str:
            t = r.choice(texts) if r.random() < 0.7 else rword(r)
            roll = r.random()
            if roll < 0.2:
                t = t.upper()
            elif roll < 0.35:
                t = " " + t + "\t"
            elif roll < 0.5:
                t = t + r.choice(texts)
            elif roll < 0.6:
                t = r.choice(texts) + t
            return t

        for _ in range(r.randint(8, 30)):
            call = r.choice(["match", "match_group", "match_html", "match_search", "match_all", "match_tokens", "scan", "item", "items_with_prefix", "pattern_count", "section"])
            v: dict[str, Any] = {"call": call}
            if index_name != "main":
                v["index"] = index_name
            g = r.choice(groups)
            if call == "match":
                v["head"] = r.choice(HEADER_NAMES) + ": " + probe()
            elif call == "match_group":
                v.update(group=g, value=probe())
                if r.random() < 0.5:
                    v["name"] = r.choice(HEADER_NAMES + [""])
            elif call == "match_html":
                v["html"] = rhtml(r)
            elif call == "match_search":
                v["search"] = {r.choice(groups): r.choice([probe(), [probe(), probe()], {r.choice(HEADER_NAMES): probe()}]) for _ in range(r.randint(1, 3))}
            elif call == "match_all":
                v.update(group=g, values=r.choice([probe(), [probe() for _ in range(3)], {r.choice(HEADER_NAMES): probe()}, 5]))
            elif call == "match_tokens":
                v.update(group=g, tokens=r.choice([[probe() for _ in range(r.randint(0, 6))], probe()]))
            elif call == "scan":
                v.update(group=g, text=" ".join(probe() for _ in range(r.randint(1, 6))))
            elif call == "item":
                v["key"] = r.choice(keys) if r.random() < 0.8 else rword(r)
            elif call == "items_with_prefix":
                k = r.choice(keys)
                v["prefix"] = k[: r.randint(0, len(k))]
            elif call == "pattern_count":
                if not index.items:
                    continue
                v["item_id"] = r.randrange(len(index.items))
                if r.random() < 0.6:
                    v["group"] = r.choice(groups)
            elif call == "section":
                names = db.section_names + ["missing"]
                v["name"] = r.choice(names)
            v["expect"] = run(db, v)
            out.append(v)
    return out


def rbuild(r: random.Random) -> dict[str, Any]:
    build: dict[str, Any] = {}
    groups = rgroups(r)
    if r.random() < 0.99:
        build["rules"] = rrules(r, groups)
        if groups or r.random() < 0.1:
            build["groups"] = groups
    if r.random() < 0.25:
        build["indexes"] = {}
        for _ in range(r.randint(1, 2)):
            ig = rgroups(r)
            build["indexes"][r.choice(["functions", "components", "x.y"] * 10 + ["main", "bad/n"])] = {"rules": rrules(r, ig), "groups": ig}
    if r.random() < 0.3:
        build["sections"] = {
            r.choice(["prints", "meta", "blob", "x"] * 8 + ["a b"]): (bytes(r.randrange(256) for _ in range(r.randint(0, 9))) if r.random() < 0.4 else rjson(r))
            for _ in range(r.randint(1, 3))
        }
    if r.random() < 0.4:
        build["metadata"] = {rword(r): rjson(r) for _ in range(r.randint(0, 3))}
    if r.random() < 0.2:
        build["timestamp"] = r.choice([0, 86_400, 1_784_237_882, 253_402_300_799])
    return build


def to_file(value: Any) -> Any:
    if isinstance(value, bytes):
        return {"hex": value.hex()}
    if isinstance(value, dict):
        return {k: to_file(v) for k, v in value.items()}
    if isinstance(value, list):
        return [to_file(v) for v in value]
    return value


def verify(out: Path) -> None:
    failures: list[str] = []
    runs = 0
    for case in sorted(p for p in out.iterdir() if p.is_dir()):
        if not (case / "vectors.json").is_file():
            continue
        vectors = json.loads((case / "vectors.json").read_text(encoding="utf-8"))
        for name in ("rs.sigdb", "ts.sigdb"):
            path = case / name
            if not path.is_file():
                continue
            db = load_bytes(path.read_bytes())
            for i, v in enumerate(vectors):
                query = {k: x for k, x in v.items() if k != "expect"}
                actual = json.loads(json.dumps(run(db, query), ensure_ascii=False))
                runs += 1
                if actual != v["expect"]:
                    failures.append(f"{case.name}/{name}[{i}] {json.dumps(query, ensure_ascii=False)[:200]}\n  expected {json.dumps(v['expect'], ensure_ascii=False)[:200]}\n  actual   {json.dumps(actual, ensure_ascii=False)[:200]}")
    print(f"{runs} vector runs on rs.sigdb and ts.sigdb")
    if failures:
        print("\n".join(failures[:30]), file=sys.stderr)
        sys.exit(f"{len(failures)} mismatches")
    print("all identical")


def main(argv: list[str]) -> None:
    if argv and argv[0] == "--verify":
        verify(Path(argv[1]))
        return
    if "--no-new-unicode" in argv:
        WORDS[:] = [w for w in WORDS if not any(unicodedata.category(c) == "Cn" for c in w)]
    out = Path(argv[0])
    count = int(argv[argv.index("--cases") + 1]) if "--cases" in argv else 500
    seed = int(argv[argv.index("--seed") + 1]) if "--seed" in argv else 1
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True)
    r = random.Random(seed)
    stats = {"built": 0, "errors": 0, "vectors": 0}
    for n in range(count):
        build = rbuild(r)
        case = out / f"{n:05d}"
        case.mkdir()
        (case / "build.json").write_text(json.dumps(to_file(build), ensure_ascii=False), encoding="utf-8")
        try:
            data = build_bytes(**build)
        except Error as e:
            stats["errors"] += 1
            (case / "error.json").write_text(json.dumps({"error": type(e).__name__, "message": str(e)}, ensure_ascii=False), encoding="utf-8")
            continue
        stats["built"] += 1
        (case / "py.sigdb").write_bytes(data)
        vectors = rvectors(r, load_bytes(data), build)
        stats["vectors"] += len(vectors)
        (case / "vectors.json").write_text(json.dumps(vectors, ensure_ascii=False), encoding="utf-8")
    print(f"{count} cases: {stats}")


if __name__ == "__main__":
    main(sys.argv[1:])
