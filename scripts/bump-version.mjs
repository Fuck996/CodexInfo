import fs from "node:fs";
import path from "node:path";

const root = process.cwd();

function readText(relativePath) {
  return fs.readFileSync(path.join(root, relativePath), "utf8");
}

function writeText(relativePath, value) {
  fs.writeFileSync(path.join(root, relativePath), `${value.replace(/\s+$/u, "")}\n`);
}

function parseVersion(value) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(value);
  if (!match) {
    throw new Error(`Unsupported version: ${value}`);
  }
  return match.slice(1).map(Number);
}

function nextPatchVersion(value) {
  const [major, minor, patch] = parseVersion(value);
  return `${major}.${minor}.${patch + 1}`;
}

function replaceJsonVersion(relativePath, nextVersion) {
  JSON.parse(readText(relativePath));
  const source = readText(relativePath);
  const next = source.replace(/("version"\s*:\s*")([^"]+)(")/u, `$1${nextVersion}$3`);
  if (next === source) {
    throw new Error(`Unable to update ${relativePath} version.`);
  }
  writeText(relativePath, next);
}

function replacePackageLockVersion(nextVersion) {
  const data = JSON.parse(readText("package-lock.json"));
  data.version = nextVersion;
  if (data.packages?.[""]) {
    data.packages[""].version = nextVersion;
  }
  writeText("package-lock.json", JSON.stringify(data, null, 2));
}

function replaceCargoPackageVersion(nextVersion) {
  const source = readText("src-tauri/Cargo.toml");
  const next = source.replace(/(\[package\][\s\S]*?\nversion\s*=\s*")([^"]+)(")/u, `$1${nextVersion}$3`);
  if (next === source) {
    throw new Error("Unable to update src-tauri/Cargo.toml package version.");
  }
  writeText("src-tauri/Cargo.toml", next);
}

const packageJson = JSON.parse(readText("package.json"));
const nextVersion = nextPatchVersion(packageJson.version);

replaceJsonVersion("package.json", nextVersion);
replacePackageLockVersion(nextVersion);
replaceJsonVersion("src-tauri/tauri.conf.json", nextVersion);
replaceCargoPackageVersion(nextVersion);

console.log(`Version bumped to ${nextVersion}`);
