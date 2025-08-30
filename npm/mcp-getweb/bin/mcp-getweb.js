#!/usr/bin/env node
// ESM launcher proxy. Detect platform/arch and spawn the right binary.

import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

/** Map Node.js platform/arch to target triple string. */
function resolveTargetTriple(platform, arch) {
  if (platform === "darwin") {
    if (arch === "arm64") return "aarch64-apple-darwin";
    if (arch === "x64") return "x86_64-apple-darwin";
    return null;
  }
  if (platform === "linux" || platform === "android") {
    if (arch === "arm64") return "aarch64-unknown-linux-musl";
    if (arch === "x64") return "x86_64-unknown-linux-musl";
    return null;
  }
  if (platform === "win32") {
    if (arch === "x64") return "x86_64-pc-windows-msvc";
    return null;
  }
  return null;
}

const target = resolveTargetTriple(process.platform, process.arch);
if (!target) {
  console.error(`Unsupported platform: ${process.platform} (${process.arch})`);
  process.exit(1);
}

const binaryName = process.platform === "win32" ? `mcp-getweb-${target}.exe` : `mcp-getweb-${target}`;
const binaryPath = path.join(__dirname, "..", binaryName.startsWith("mcp-getweb-") ? "bin" : "bin", binaryName);

const child = spawn(binaryPath, process.argv.slice(2), { stdio: "inherit" });
child.on("error", (err) => {
  // Print stack and exit with code 1 if binary missing or not executable.
  console.error(err);
  process.exit(1);
});

const forward = (sig) => {
  if (!child.killed) {
    try {
      child.kill(sig);
    } catch {
      // ignore
    }
  }
};

["SIGINT", "SIGTERM", "SIGHUP"].forEach((s) => {
  process.on(s, () => forward(s));
});

child.on("exit", (code, signal) => {
  if (signal) {
    // Mirror child's termination signal
    try {
      process.kill(process.pid, signal);
    } catch {
      process.exit(1);
    }
    return;
  }
  process.exit(code ?? 1);
});
