import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function readJson(relativePath) {
  return JSON.parse(fs.readFileSync(path.join(repoRoot, relativePath), "utf8"));
}

function fail(message) {
  console.error(`release-check: ${message}`);
  process.exitCode = 1;
}

function argValue(name) {
  const index = process.argv.indexOf(name);
  if (index === -1) {
    return undefined;
  }

  return process.argv[index + 1];
}

function hasFlag(name) {
  return process.argv.includes(name);
}

const packageJson = readJson("package.json");
const packageLock = readJson("package-lock.json");
const tauriConfig = readJson("src-tauri/tauri.conf.json");
const cargoToml = fs.readFileSync(path.join(repoRoot, "src-tauri", "Cargo.toml"), "utf8");
const cargoVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const rootLockVersion = packageLock.packages?.[""]?.version;

const versions = {
  "package.json": packageJson.version,
  "package-lock.json": rootLockVersion,
  "src-tauri/tauri.conf.json": tauriConfig.version,
  "src-tauri/Cargo.toml": cargoVersion,
};

for (const [file, version] of Object.entries(versions)) {
  if (!version) {
    fail(`${file} does not expose a release version`);
  }
}

const uniqueVersions = new Set(Object.values(versions));
if (uniqueVersions.size !== 1) {
  fail(`version mismatch: ${JSON.stringify(versions)}`);
}

const version = packageJson.version;
const tag = argValue("--tag") ?? process.env.RELEASE_TAG;
if (tag && tag !== `v${version}`) {
  fail(`release tag ${tag} does not match v${version}`);
}

if (!tauriConfig.identifier || tauriConfig.identifier === "dev.local.cedar" || tauriConfig.identifier.includes(".local.")) {
  fail(`release builds need a stable production identifier, got ${tauriConfig.identifier}`);
}

if (!Array.isArray(tauriConfig.bundle?.icon) || !tauriConfig.bundle.icon.includes("icons/icon.icns")) {
  fail("Tauri bundle icons must include icons/icon.icns for macOS releases");
}

if (!packageJson.scripts?.["desktop:build:no-bundle"]) {
  fail("package.json is missing desktop:build:no-bundle for the Windows portable release lane");
}

if (hasFlag("--check-workflow")) {
  const workflowPath = path.join(repoRoot, ".github", "workflows", "release.yml");
  if (!fs.existsSync(workflowPath)) {
    fail(".github/workflows/release.yml is missing");
  } else {
    const workflow = fs.readFileSync(workflowPath, "utf8");
    const requiredSignals = [
      "workflow_dispatch:",
      "ref: ${{ github.event_name == 'workflow_dispatch' && inputs.tag || github.ref }}",
      "tags:",
      "v*",
      "windows-latest",
      "macos-latest",
      "SHA256SUMS-windows.txt",
      "SHA256SUMS-macos.txt",
      "contents: write",
      "ENIGMA_VIRTUAL_BOX_INSTALLER_URL",
      "APPLE_CERTIFICATE",
      "APPLE_TEAM_ID",
    ];

    for (const signal of requiredSignals) {
      if (!workflow.includes(signal)) {
        fail(`release workflow is missing ${signal}`);
      }
    }
  }
}

if (process.exitCode) {
  process.exit(process.exitCode);
}

console.log(`release-check: Cedar ${version} metadata is release-aligned`);
