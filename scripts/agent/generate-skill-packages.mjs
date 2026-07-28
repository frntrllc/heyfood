#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const source = path.join(root, "agent-integrations", "skills", "heyfood");
const destinations = [
  path.join(root, "agent-integrations", "codex", "heyfood", "skills", "heyfood"),
  path.join(root, "agent-integrations", "claude", "heyfood", "skills", "heyfood"),
];
const files = [
  "SKILL.md",
  "agents/openai.yaml",
  "references/authentication-and-capabilities.md",
  "references/grocery.md",
  "references/safety-and-recovery.md",
  "references/workflow-selection.md",
];
const check = process.argv.slice(2).includes("--check");

if (process.argv.length > 3 || (process.argv.length === 3 && !check)) {
  process.stderr.write("usage: generate-skill-packages.mjs [--check]\n");
  process.exit(2);
}

for (const destination of destinations) {
  const existing = fs.existsSync(destination)
    ? fs
        .readdirSync(destination, { recursive: true, withFileTypes: true })
        .filter((entry) => entry.isFile())
        .map((entry) =>
          path
            .relative(destination, path.join(entry.parentPath, entry.name))
            .split(path.sep)
            .join("/"),
        )
        .sort()
    : [];
  const unexpected = existing.filter((relative) => !files.includes(relative));
  if (unexpected.length > 0) {
    throw new Error(
      `refusing to replace unexpected packaged skill files: ${unexpected.join(", ")}`,
    );
  }

  for (const relative of files) {
    const sourcePath = path.join(source, relative);
    const destinationPath = path.join(destination, relative);
    const sourceBytes = fs.readFileSync(sourcePath);
    const destinationBytes = fs.existsSync(destinationPath)
      ? fs.readFileSync(destinationPath)
      : null;
    if (check) {
      if (destinationBytes === null || !sourceBytes.equals(destinationBytes)) {
        throw new Error(`packaged skill drift: ${path.relative(root, destinationPath)}`);
      }
      continue;
    }
    if (destinationBytes !== null && sourceBytes.equals(destinationBytes)) {
      continue;
    }
    fs.mkdirSync(path.dirname(destinationPath), { recursive: true });
    const staging = `${destinationPath}.${process.pid}.tmp`;
    fs.writeFileSync(staging, sourceBytes, { flag: "wx", mode: 0o644 });
    fs.renameSync(staging, destinationPath);
  }
}

process.stdout.write(
  `${check ? "verified" : "generated"} ${files.length} canonical skill files in ${destinations.length} host packages\n`,
);
