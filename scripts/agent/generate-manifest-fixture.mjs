#!/usr/bin/env node

import childProcess from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const executable = path.join(
  root,
  "target",
  "debug",
  process.platform === "win32" ? "heyfood.exe" : "heyfood",
);
const fixture = path.join(root, "fixtures", "agent", "manifest-v1-golden.json");
const check = process.argv.slice(2).includes("--check");

if (process.argv.length > 3 || (process.argv.length === 3 && !check)) {
  process.stderr.write("usage: generate-manifest-fixture.mjs [--check]\n");
  process.exit(2);
}

if (!fs.existsSync(executable)) {
  throw new Error(`build heyfood-bin before generating the manifest: ${executable}`);
}
const stdout = childProcess.execFileSync(
  executable,
  ["agent", "describe", "--schema-version", "1"],
  {
    cwd: root,
    encoding: "utf8",
    env: { ...process.env, NO_COLOR: "1" },
    maxBuffer: 2 * 1024 * 1024,
  },
);
const document = JSON.parse(stdout);
if (fs.existsSync(fixture)) {
  const checkedIn = JSON.parse(fs.readFileSync(fixture, "utf8"));
  document.build = checkedIn.build;
}
const generated = `${JSON.stringify(document, null, 2)}\n`;
if (check) {
  const checkedIn = fs.readFileSync(fixture, "utf8").replaceAll("\r\n", "\n");
  if (checkedIn !== generated) {
    throw new Error("checked-in agent manifest fixture has drifted");
  }
} else {
  const staging = `${fixture}.${process.pid}.tmp`;
  fs.writeFileSync(staging, generated, { flag: "wx", mode: 0o644 });
  fs.renameSync(staging, fixture);
}
process.stdout.write(`${check ? "verified" : "generated"} ${path.relative(root, fixture)}\n`);
