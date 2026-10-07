#!/usr/bin/env node
// package-lock.json as bitbake's npmsw fetcher can take it: bitbake-lock.json,
// what the tessaro-demo recipe fetches. The same rules as Webconfig's copy
// (webconfig/scripts/bitbake-lock.mjs, docs/webconfig.md, "Built in
// bitbake"):
//
// npmsw fetches every package of the lock, whatever its os and cpu, and
// refuses an entry without an integrity - which is what a package bundled
// inside another (`inBundle`) has. So this keeps what a Linux build host on
// x86_64 or arm64 can run, and drops the rest:
//
// * bundled packages, which their parent's tarball already carries;
// * packages for another os or cpu, and the musl and wasm32 variants,
//   which a glibc host never loads.
//
// `--check` fails when bitbake-lock.json is not what package-lock.json makes,
// or when a kept package has nothing npmsw can fetch it from.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const source = join(root, "package-lock.json");
const target = join(root, "bitbake-lock.json");

const HOST_OS = "linux";
const HOST_CPUS = ["x64", "arm64"];

/** Whether a package's `os` or `cpu` list lets `wanted` in. */
function allows(list, wanted) {
  if (!Array.isArray(list) || list.length === 0) return true;
  const denied = list.filter((entry) => entry.startsWith("!")).map((entry) => entry.slice(1));
  const allowed = list.filter((entry) => !entry.startsWith("!"));
  return wanted.some((one) => !denied.includes(one) && (allowed.length === 0 || allowed.includes(one)));
}

function keep(path, entry) {
  if (path === "") return true;
  if (entry.inBundle) return false;
  if (!allows(entry.os, [HOST_OS])) return false;
  if (!allows(entry.cpu, HOST_CPUS)) return false;
  const name = path.split("node_modules/").pop();
  return !/(^|[-/])(musl|wasm32)([-/]|$)/.test(name) && !name.includes("-musl") && !name.includes("wasm32");
}

const lock = JSON.parse(readFileSync(source, "utf8"));
const packages = {};
const problems = [];
for (const [path, entry] of Object.entries(lock.packages ?? {})) {
  if (!keep(path, entry)) continue;
  if (path !== "" && (!entry.resolved || !entry.integrity)) {
    problems.push(`${path}: no resolved or integrity for npmsw to fetch`);
  }
  packages[path] = entry;
}
const made = `${JSON.stringify({ ...lock, packages }, null, 2)}\n`;

if (process.argv.includes("--check")) {
  let current = "";
  try {
    current = readFileSync(target, "utf8");
  } catch {
    problems.push("bitbake-lock.json is missing");
  }
  if (current && current !== made) {
    problems.push("bitbake-lock.json is out of date; run `mise run demo:lock`");
  }
  if (problems.length > 0) {
    console.error(problems.join("\n"));
    process.exit(1);
  }
} else {
  if (problems.length > 0) {
    console.error(problems.join("\n"));
    process.exit(1);
  }
  writeFileSync(target, made);
  const dropped = Object.keys(lock.packages).length - Object.keys(packages).length;
  console.log(`bitbake-lock.json: ${Object.keys(packages).length - 1} packages, ${dropped} left out`);
}
