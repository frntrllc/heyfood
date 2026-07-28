#!/usr/bin/env node

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import readline from "node:readline";

const [binary, ...extra] = process.argv.slice(2);
if (!binary || extra.length !== 0) {
  console.error("usage: mcp-smoke.mjs /absolute/path/to/heyfood");
  process.exit(64);
}

const environment = { ...process.env };
for (const name of Object.keys(environment)) {
  if (name.startsWith("HEYFOOD_")) {
    delete environment[name];
  }
}
const cleanHome = mkdtempSync(join(tmpdir(), "heyfood-mcp-smoke-"));
process.on("exit", () => {
  rmSync(cleanHome, { recursive: true, force: true });
});
environment.HOME = cleanHome;
environment.USERPROFILE = cleanHome;
environment.XDG_CONFIG_HOME = join(cleanHome, "config");
environment.XDG_DATA_HOME = join(cleanHome, "data");
environment.XDG_CACHE_HOME = join(cleanHome, "cache");
environment.APPDATA = join(cleanHome, "appdata");
environment.LOCALAPPDATA = join(cleanHome, "localappdata");

const child = spawn(binary, ["mcp", "serve"], {
  env: environment,
  stdio: ["pipe", "pipe", "pipe"],
});
let stderr = "";
child.stderr.setEncoding("utf8");
child.stderr.on("data", (chunk) => {
  if (stderr.length < 4096) {
    stderr += chunk;
  }
});

let nextId = 1;
const pending = new Map();
const lines = readline.createInterface({ input: child.stdout, crlfDelay: Infinity });
lines.on("line", (line) => {
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    fail("MCP stdout contained a non-JSON frame");
    return;
  }
  const waiter = pending.get(message.id);
  if (waiter) {
    pending.delete(message.id);
    waiter.resolve(message);
  }
});

function fail(message) {
  for (const waiter of pending.values()) {
    clearTimeout(waiter.timer);
    waiter.reject(new Error(message));
  }
  pending.clear();
  child.kill("SIGTERM");
}

function request(method, params) {
  const id = nextId++;
  child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`timed out waiting for ${method}`));
    }, 20_000);
    pending.set(id, {
      timer,
      resolve: (message) => {
        clearTimeout(timer);
        resolve(message);
      },
      reject,
    });
  });
}

function structured(message) {
  return message?.result?.structuredContent;
}

async function waitForExit() {
  if (child.exitCode !== null) {
    if (child.exitCode === 0 && child.signalCode === null) {
      return;
    }
    throw new Error(
      `MCP process exited with code=${child.exitCode} signal=${child.signalCode}`,
    );
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error("MCP process did not exit within five seconds of EOF"));
    }, 5_000);
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("exit", (code, signal) => {
      clearTimeout(timer);
      if (code === 0 && signal === null) {
        resolve();
      } else {
        reject(new Error(`MCP process exited with code=${code} signal=${signal}`));
      }
    });
  });
}

try {
  const initialized = await request("initialize", {
    protocolVersion: "2025-11-25",
    capabilities: {},
    clientInfo: { name: "heyfood-release-smoke", version: "1" },
  });
  if (initialized.error || initialized.result?.protocolVersion !== "2025-11-25") {
    throw new Error("MCP initialization contract mismatch");
  }
  child.stdin.write(
    `${JSON.stringify({
      jsonrpc: "2.0",
      method: "notifications/initialized",
      params: {},
    })}\n`,
  );

  const listed = await request("tools/list", {});
  const names = listed.result?.tools?.map((tool) => tool.name);
  const expected = [
    "heyfood_get_manifest",
    "heyfood_get_status",
    "heyfood_get_capabilities",
    "heyfood_get_grocery_list",
    "heyfood_get_grocery_exclusions",
    "heyfood_list_menu_watches",
  ];
  if (JSON.stringify(names) !== JSON.stringify(expected)) {
    throw new Error("MCP tool allowlist mismatch");
  }
  for (const tool of listed.result.tools) {
    if (
      tool.inputSchema?.additionalProperties !== false ||
      tool.inputSchema?.type !== "object" ||
      !tool.outputSchema
    ) {
      throw new Error(`MCP schema is not closed for ${tool.name}`);
    }
  }

  const manifest = await request("tools/call", {
    name: "heyfood_get_manifest",
    arguments: {},
  });
  if (
    manifest.result?.isError !== false ||
    structured(manifest)?.schema_version !== 1 ||
    structured(manifest)?.automation_surfaces?.mcp_stdio !== "active"
  ) {
    throw new Error("installed MCP manifest call failed");
  }

  const invalid = await request("tools/call", {
    name: "heyfood_get_manifest",
    arguments: { forbidden: true },
  });
  if (invalid.error?.code !== -32602) {
    throw new Error("MCP accepted a non-empty argument object");
  }

  const missingAuthentication = await request("tools/call", {
    name: "heyfood_get_status",
    arguments: {},
  });
  if (
    missingAuthentication.result?.isError !== true ||
    typeof structured(missingAuthentication)?.error?.code !== "string"
  ) {
    throw new Error("clean-profile MCP auth failure was not typed");
  }
  if (
    process.platform === "darwin" &&
    structured(missingAuthentication).error.code !== "login_required"
  ) {
    throw new Error(
      `signed macOS clean profile returned ${structured(missingAuthentication).error.code} instead of login_required`,
    );
  }

  child.stdin.end();
  await waitForExit();
  if (stderr !== "") {
    throw new Error("MCP emitted diagnostics during the clean protocol smoke");
  }
  console.log("MCP installed-artifact smoke passed: 6 tools, closed schemas, typed auth handoff.");
} catch (error) {
  child.kill("SIGTERM");
  const diagnostic = stderr.replace(/[^\x20-\x7e\n]/g, "?").trim();
  console.error(`MCP installed-artifact smoke failed: ${error.message}`);
  if (diagnostic) {
    console.error(`server diagnostic: ${diagnostic}`);
  }
  process.exit(1);
}
