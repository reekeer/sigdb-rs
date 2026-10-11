// Runs the vectors of scripts/crosscheck.py cases through sigdb-ts, on the file the Python package
// built (py.sigdb) and on the one sigdb-rs built (rs.sigdb, written by examples/crosscheck.rs),
// and builds the same rules with sigdb-ts into ts.sigdb for examples/crosscheck.rs to read.
//
//     SIGDB_TS=../sigdb-ts bun scripts/crosscheck-ts.ts <cases dir>

import { readFileSync, readdirSync, existsSync } from "node:fs";
import { join, resolve } from "node:path";

const tsRoot = resolve(process.env.SIGDB_TS ?? "../sigdb-ts");
const { loadBytes } = await import(join(tsRoot, "src/index.ts"));
const { parseJsonBytes } = await import(join(tsRoot, "src/internal/json.ts"));
const { runVector, toBuildArgs } = await import(join(tsRoot, "tests/support.ts"));
const { buildBytes } = await import(join(tsRoot, "src/index.ts"));
const { writeFileSync } = await import("node:fs");

function same(a: unknown, b: unknown): boolean {
  if (typeof a === "bigint" || typeof b === "bigint") {
    return String(a) === String(b);
  }
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") {
    return a === b || (Number.isNaN(a) && Number.isNaN(b));
  }
  if (Array.isArray(a) || Array.isArray(b)) {
    return Array.isArray(a) && Array.isArray(b) && a.length === b.length && a.every((x, i) => same(x, b[i]));
  }
  const ea = a instanceof Map ? [...a.entries()] : Object.entries(a as object);
  const eb = new Map(b instanceof Map ? [...b.entries()] : Object.entries(b as object));
  return ea.length === eb.size && ea.every(([k, v]) => eb.has(k) && same(v, eb.get(k)));
}

const dir = process.argv[2];
if (!dir) throw new Error("usage: crosscheck-ts.ts <cases dir>");
let checked = 0;
let vectors = 0;
const failures: string[] = [];
for (const name of readdirSync(dir).sort()) {
  const caseDir = join(dir, name);
  if (!existsSync(join(caseDir, "rs.sigdb"))) continue;
  const list = parseJsonBytes(readFileSync(join(caseDir, "vectors.json")), "map") as Map<string, unknown>[];
  const expects = parseJsonBytes(readFileSync(join(caseDir, "vectors.json")), "object") as { expect: unknown }[];
  for (const file of ["py.sigdb", "rs.sigdb"]) {
    const db = loadBytes(readFileSync(join(caseDir, file)));
    list.forEach((input, i) => {
      const actual = runVector(db, input);
      if (!same(actual, expects[i]?.expect)) {
        failures.push(`${name}/${file}[${i}] ${JSON.stringify(Object.fromEntries(input), (_, v) => (typeof v === "bigint" ? String(v) : v)).slice(0, 300)}\n  expected ${JSON.stringify(expects[i]?.expect, (_, v) => (typeof v === "bigint" ? String(v) : v)).slice(0, 300)}\n  actual   ${JSON.stringify(actual, (_, v) => (typeof v === "bigint" ? String(v) : v)).slice(0, 300)}`);
      }
    });
    vectors += list.length;
  }
  const args = toBuildArgs(parseJsonBytes(readFileSync(join(caseDir, "build.json")), "map"));
  writeFileSync(join(caseDir, "ts.sigdb"), buildBytes(args.rules, args.options));
  checked++;
}
console.log(`${checked} cases, ${vectors} vector runs`);
if (failures.length) {
  console.error(failures.slice(0, 30).join("\n"));
  console.error(`${failures.length} mismatches`);
  process.exit(1);
}
console.log("all identical");
