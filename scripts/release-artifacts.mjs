import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

function argValue(name, fallback) {
  const index = process.argv.indexOf(name);
  if (index === -1) {
    return fallback;
  }

  return process.argv[index + 1];
}

function fail(message) {
  console.error(`release-artifacts: ${message}`);
  process.exit(1);
}

function sha256(filePath) {
  const hash = crypto.createHash("sha256");
  hash.update(fs.readFileSync(filePath));
  return hash.digest("hex");
}

function fileRows(dir, manifestName) {
  return fs
    .readdirSync(dir, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name !== manifestName)
    .map((entry) => entry.name)
    .sort((a, b) => a.localeCompare(b))
    .map((name) => {
      const fullPath = path.join(dir, name);
      return `${sha256(fullPath)}  ${name}`;
    });
}

function requiredFiles(platform, version) {
  if (platform === "windows") {
    return [
      `Cedar_${version}_windows-x64-portable.exe`,
      `Cedar_${version}_windows-x64.zip`,
      "SHA256SUMS-windows.txt",
    ];
  }

  if (platform === "macos") {
    return [`Cedar_${version}_macos.app.zip`, `Cedar_${version}_macos.dmg`, "SHA256SUMS-macos.txt"];
  }

  fail(`unknown platform ${platform}`);
}

function writeManifest(dir, manifestName) {
  fs.mkdirSync(dir, { recursive: true });
  const rows = fileRows(dir, manifestName);
  if (!rows.length) {
    fail(`no files found for checksum manifest in ${dir}`);
  }

  fs.writeFileSync(path.join(dir, manifestName), `${rows.join("\n")}\n`);
}

function verifyManifest(dir, manifestName) {
  const manifestPath = path.join(dir, manifestName);
  if (!fs.existsSync(manifestPath)) {
    fail(`${manifestName} is missing`);
  }

  const rows = fs
    .readFileSync(manifestPath, "utf8")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);

  for (const row of rows) {
    const match = row.match(/^([a-fA-F0-9]{64})\s+\*?(.+)$/);
    if (!match) {
      fail(`${manifestName} has malformed row: ${row}`);
    }

    const [, expectedHash, fileName] = match;
    const filePath = path.join(dir, fileName);
    if (!fs.existsSync(filePath)) {
      fail(`${fileName} listed in ${manifestName} is missing`);
    }

    const actualHash = sha256(filePath);
    if (actualHash.toLowerCase() !== expectedHash.toLowerCase()) {
      fail(`${fileName} checksum mismatch`);
    }
  }
}

function verifyRequiredFiles(dir, platform, version) {
  for (const fileName of requiredFiles(platform, version)) {
    const filePath = path.join(dir, fileName);
    if (!fs.existsSync(filePath)) {
      fail(`${fileName} is missing`);
    }

    if (fileName !== "SHA256SUMS-windows.txt" && fileName !== "SHA256SUMS-macos.txt") {
      const bytes = fs.statSync(filePath).size;
      if (bytes < 1024 * 1024) {
        fail(`${fileName} is implausibly small (${bytes} bytes)`);
      }
    }
  }
}

const command = process.argv[2];
const platform = argValue("--platform");
const dir = path.resolve(argValue("--dir", "."));
const version = argValue("--version");

if (!platform) {
  fail("--platform is required");
}

const manifestName = platform === "windows" ? "SHA256SUMS-windows.txt" : "SHA256SUMS-macos.txt";

if (command === "checksum") {
  writeManifest(dir, manifestName);
  verifyManifest(dir, manifestName);
  console.log(`release-artifacts: wrote ${path.join(dir, manifestName)}`);
} else if (command === "verify") {
  if (!version) {
    fail("--version is required for verify");
  }

  verifyRequiredFiles(dir, platform, version);
  verifyManifest(dir, manifestName);
  console.log(`release-artifacts: ${platform} artifacts verified`);
} else {
  fail("expected command checksum or verify");
}
