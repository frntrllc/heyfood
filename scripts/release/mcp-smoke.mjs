#!/usr/bin/env node

import { randomBytes } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";
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
let ownedMacKeychain;
process.on("exit", () => {
  if (ownedMacKeychain && existsSync(ownedMacKeychain)) {
    const deleted = spawnSync(
      "/usr/bin/security",
      ["delete-keychain", ownedMacKeychain],
      {
        env: { ...process.env, HOME: cleanHome },
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    if (deleted.status !== 0 || existsSync(ownedMacKeychain)) {
      console.error("MCP smoke could not destroy its ephemeral macOS keychain");
      process.exitCode = 1;
    }
  }
  rmSync(cleanHome, { recursive: true, force: true });
});
environment.HOME = cleanHome;
environment.USERPROFILE = cleanHome;
environment.XDG_CONFIG_HOME = join(cleanHome, "config");
environment.XDG_DATA_HOME = join(cleanHome, "data");
environment.XDG_CACHE_HOME = join(cleanHome, "cache");
environment.APPDATA = join(cleanHome, "appdata");
environment.LOCALAPPDATA = join(cleanHome, "localappdata");

function runSecurity(arguments_) {
  const result = spawnSync("/usr/bin/security", arguments_, {
    env: { ...process.env, HOME: cleanHome },
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.status !== 0) {
    throw new Error(
      "could not configure the isolated macOS credential container",
    );
  }
}

function configureMacCredentialContainer() {
  if (process.platform !== "darwin") {
    return;
  }
  mkdirSync(join(cleanHome, "Library", "Preferences"), {
    recursive: true,
    mode: 0o700,
  });
  mkdirSync(join(cleanHome, "Library", "Keychains"), {
    recursive: true,
    mode: 0o700,
  });
  const externalKeychain = process.env.HEYFOOD_QUALIFICATION_KEYCHAIN;
  let keychain = externalKeychain;
  if (keychain) {
    if (!isAbsolute(keychain) || !existsSync(keychain)) {
      throw new Error("external macOS qualification keychain is unavailable");
    }
  } else {
    keychain = join(
      cleanHome,
      "Library",
      "Keychains",
      "heyfood-mcp-smoke.keychain-db",
    );
    ownedMacKeychain = keychain;
    const password = randomBytes(32).toString("hex");
    runSecurity(["create-keychain", "-p", password, keychain]);
    runSecurity(["set-keychain-settings", "-lut", "21600", keychain]);
    runSecurity(["unlock-keychain", "-p", password, keychain]);
  }
  runSecurity(["default-keychain", "-d", "user", "-s", keychain]);
  runSecurity(["list-keychains", "-d", "user", "-s", keychain]);
}

try {
  configureMacCredentialContainer();
} catch (error) {
  console.error(`MCP installed-artifact smoke failed: ${error.message}`);
  process.exit(1);
}

function loadExpectedManifest() {
  const result = spawnSync(binary, ["agent", "describe"], {
    env: environment,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 2 * 1024 * 1024,
  });
  if (result.status !== 0 || result.signal !== null) {
    throw new Error("could not read the installed binary agent manifest");
  }
  const manifest = JSON.parse(result.stdout);
  const names = manifest?.mcp_inventory?.tools?.map((tool) => tool.name);
  if (
    !Number.isSafeInteger(manifest?.schema_version) ||
    manifest.schema_version < 3 ||
    manifest?.automation_surfaces?.mcp_stdio !== "active" ||
    !Array.isArray(names) ||
    names.length === 0 ||
    new Set(names).size !== names.length
  ) {
    throw new Error("installed binary agent manifest is not release-smoke compatible");
  }
  if (
    manifest.schema_version >= 4 &&
    (!names.includes("heyfood_list_diets") ||
      !names.includes("heyfood_get_diet") ||
      !manifest.capabilities?.some(
        (capability) =>
          capability.id === "diet-guidance" && capability.status === "active",
      ))
  ) {
    throw new Error("schema-v4 agent manifest omitted the Diet read surface");
  }
  return { manifest, names };
}

let installedDiscovery;
try {
  installedDiscovery = loadExpectedManifest();
} catch (error) {
  console.error(`MCP installed-artifact smoke failed: ${error.message}`);
  process.exit(1);
}
const expectedManifest = installedDiscovery.manifest;
const expectedToolNames = installedDiscovery.names;

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
const lines = readline.createInterface({
  input: child.stdout,
  crlfDelay: Infinity,
});
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
  child.stdin.write(
    `${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`,
  );
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
        reject(
          new Error(`MCP process exited with code=${code} signal=${signal}`),
        );
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
  if (
    initialized.error ||
    initialized.result?.protocolVersion !== "2025-11-25"
  ) {
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
  if (JSON.stringify(names) !== JSON.stringify(expectedToolNames)) {
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
    JSON.stringify(structured(manifest)) !== JSON.stringify(expectedManifest)
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
  if (
    structured(missingAuthentication).error.code === "login_required" &&
    structured(missingAuthentication).error.user_action !== "heyfood login"
  ) {
    throw new Error(
      "clean-profile MCP auth failure omitted the exact heyfood login handoff",
    );
  }

  child.stdin.end();
  await waitForExit();
  if (stderr !== "") {
    throw new Error("MCP emitted diagnostics during the clean protocol smoke");
  }
  console.log(
    `MCP installed-artifact smoke passed: ${expectedToolNames.length} manifest-derived tools, closed schemas, typed auth handoff.`,
  );
} catch (error) {
  child.kill("SIGTERM");
  const diagnostic = stderr.replace(/[^\x20-\x7e\n]/g, "?").trim();
  console.error(`MCP installed-artifact smoke failed: ${error.message}`);
  if (diagnostic) {
    console.error(`server diagnostic: ${diagnostic}`);
  }
  process.exit(1);
}
