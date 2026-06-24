import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const require = createRequire(import.meta.url);
const generateEvb = require("generate-evb");

function argValue(name) {
  const index = process.argv.indexOf(name);
  if (index === -1) {
    return undefined;
  }

  return process.argv[index + 1];
}

function fail(message) {
  console.error(`generate-evb: ${message}`);
  process.exit(1);
}

const project = argValue("--project");
const input = argValue("--input");
const output = argValue("--output");
const pack = argValue("--pack");

if (!project || !input || !output || !pack) {
  fail("--project, --input, --output, and --pack are required");
}

if (!fs.existsSync(input)) {
  fail(`input executable does not exist: ${input}`);
}

if (!fs.existsSync(pack)) {
  fail(`pack directory does not exist: ${pack}`);
}

fs.mkdirSync(path.dirname(project), { recursive: true });
fs.mkdirSync(path.dirname(output), { recursive: true });

const outputName = path.basename(output).toLowerCase();
generateEvb(project, input, output, pack, {
  filter(_fullPath, name) {
    const lowerName = name.toLowerCase();
    return lowerName !== outputName && !lowerName.endsWith(".pdb");
  },
  evbOptions: {
    compressFiles: true,
    deleteExtractedOnExit: true,
    mapExecutableWithTemporaryFile: true,
    allowRunningOfVirtualExeFiles: true,
  },
});

console.log(`generate-evb: wrote ${project}`);
