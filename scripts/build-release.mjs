import { spawnSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
// Stripping symbols does not remove paths in Rust panic locations and file!().
// Remap both the checkout and toolchain/cache roots before compiling any crate.
const mappings = [
  [homedir(), "/build/home"],
  [process.env.CARGO_HOME, "/build/cargo"],
  [process.env.RUSTUP_HOME, "/build/rustup"],
  [root, "/build/ai-usage-dashboard"],
].filter(([path]) => path);
const flags = process.env.CARGO_ENCODED_RUSTFLAGS !== undefined
  ? process.env.CARGO_ENCODED_RUSTFLAGS.split("\x1f").filter(Boolean)
  : (process.env.RUSTFLAGS ?? "").split(/\s+/).filter(Boolean);
for (const [path, replacement] of mappings) {
  let canonical = path;
  try { canonical = realpathSync(path); } catch { /* A cache may not exist yet. */ }
  for (const prefix of new Set([path, canonical, path.replaceAll("\\", "/"), canonical.replaceAll("\\", "/")])) {
    flags.push(`--remap-path-prefix=${prefix}=${replacement}`);
  }
}
const result = spawnSync(process.execPath, [
  require.resolve("@tauri-apps/cli/tauri.js"), "build", ...process.argv.slice(2),
], {
  cwd: root,
  env: { ...process.env, CARGO_ENCODED_RUSTFLAGS: flags.join("\x1f") },
  stdio: "inherit",
});
if (result.error) throw result.error;
process.exit(result.status ?? 1);
