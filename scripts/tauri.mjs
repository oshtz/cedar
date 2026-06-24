import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import os from "node:os";
import path from "node:path";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tauriBin =
  process.platform === "win32"
    ? path.join(repoRoot, "node_modules", ".bin", "tauri.cmd")
    : path.join(repoRoot, "node_modules", ".bin", "tauri");

const env = { ...process.env };

if (process.platform === "win32" && !env.CARGO_TARGET_DIR) {
  env.CARGO_TARGET_DIR = path.join(os.tmpdir(), "cedar-cargo-target");
}

let child;
try {
  child = spawn(tauriBin, process.argv.slice(2), {
    cwd: repoRoot,
    env,
    shell: process.platform === "win32",
    stdio: "inherit",
  });
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }

  process.exit(code ?? 1);
});

child.on("error", (error) => {
  console.error(error.message);
  process.exit(1);
});
