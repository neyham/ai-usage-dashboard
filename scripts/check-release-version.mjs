import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const root = new URL("../", import.meta.url);
const read = (path) => readFileSync(new URL(path, root), "utf8");
const json = (path) => JSON.parse(read(path));
const version = json("package.json").version;
const tag = process.argv[2];

assert.match(version, /^\d+\.\d+\.\d+$/, "Expected a stable release version");
assert.equal(tag, `v${version}`, "Release tag must match package.json");
const lock = json("package-lock.json");
assert.equal(lock.version, version, "package-lock.json version mismatch");
assert.equal(lock.packages[""].version, version, "npm root package version mismatch");
assert.equal(json("src-tauri/tauri.conf.json").version, version, "Tauri version mismatch");
const cargoPackage = read("src-tauri/Cargo.toml").split(/^\[package\]\s*$/m)[1]?.split(/^\[/m)[0];
assert.equal(cargoPackage?.match(/^version\s*=\s*"([^"]+)"/m)?.[1], version, "Cargo package version mismatch");
const cargoLockVersion = read("src-tauri/Cargo.lock").match(/\[\[package\]\]\s*\nname = "ai-usage-dashboard"\s*\nversion = "([^"]+)"/)?.[1];
assert.equal(cargoLockVersion, version, "Cargo lockfile application version mismatch");
assert.ok(read(`docs/releases/${tag}.md`).trim(), "Release notes must be present");
console.log(`PASS ${tag}: application versions, lockfiles, and release notes agree`);
