from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

from sigdb import build_bytes, load_bytes

ROOT = Path(__file__).resolve().parent.parent / "tests"


def build_args(case: Path) -> dict[str, Any]:
    build = case / "build.json"
    if build.is_file():
        args = json.loads(build.read_bytes())
    else:
        args = {"rules": json.loads((case / "rules.json").read_bytes())}
    sections = args.get("sections")
    if isinstance(sections, dict):
        for name, value in list(sections.items()):
            if isinstance(value, dict) and set(value) == {"hex"}:
                sections[name] = bytes.fromhex(value["hex"])
    return args


def main(argv: list[str]) -> None:
    check = "--check" in argv
    out_dir = ROOT / "fixtures" / "py"
    out_dir.mkdir(parents=True, exist_ok=True)
    failures = []
    for group in ("golden", "golden-extra"):
        for case in sorted(p for p in (ROOT / group).iterdir() if p.is_dir()):
            data = build_bytes(**build_args(case))
            digests = load_bytes(data).section_digests()
            expected = json.loads((case / "sections.json").read_bytes())
            if digests != expected:
                failures.append(f"{group}/{case.name}: python digests differ from sections.json")
            target = out_dir / f"{case.name}.sigdb"
            if check:
                if load_bytes(target.read_bytes()).section_digests() != digests:
                    failures.append(f"{target.name}: python build differs from the fixture")
            else:
                target.write_bytes(data)
    if failures:
        raise SystemExit("\n".join(failures))


if __name__ == "__main__":
    main(sys.argv[1:])
