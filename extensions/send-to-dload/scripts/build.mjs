// scripts/build.mjs
// Build the distributable artifacts:
//   dist/chrome/send-to-dload-<version>.zip   — Chrome / Edge / Brave / Opera
//   dist/firefox/send-to-dload-<version>.xpi  — Firefox (AMO / about:debugging)
//
// Usage:
//   node scripts/build.mjs            # both targets
//   node scripts/build.mjs chromium   # Chrome only
//   node scripts/build.mjs firefox    # Firefox only
//
// Strategy:
//   1. Stage ONLY the runtime files (manifest.json, _locales/, src/) into a temp
//      dir — no tests, configs, package.json, or scripts leak into the artifact.
//   2. Write a per-target manifest:
//        - Chromium: drop browser_specific_settings; keep background.service_worker,
//          drop background.scripts (a Firefox-only event-page key Chrome warns on).
//        - Firefox:  keep background.scripts, drop background.service_worker
//          (a Chrome-only key Firefox warns on).
//   3. Hand each staged dir to `web-ext build`.
//   4. Rename the Firefox .zip → .xpi (Firefox accepts either).

import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile, rm, cp, rename, mkdtemp } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "..");
const DIST = path.join(ROOT, "dist");

// Only these top-level items ship in the artifact.
const RUNTIME_ITEMS = ["_locales", "src"];

async function readJSON(p) {
  return JSON.parse(await readFile(p, "utf8"));
}

function run(cmd, args, opts = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { stdio: "inherit", ...opts });
    child.on("error", reject);
    child.on("exit", (code) => {
      if (code === 0) resolve();
      else reject(new Error(`${cmd} ${args.join(" ")} exited ${code}`));
    });
  });
}

async function stageRuntime(stage, manifest) {
  for (const item of RUNTIME_ITEMS) {
    await cp(path.join(ROOT, item), path.join(stage, item), { recursive: true });
  }
  await writeFile(path.join(stage, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
}

async function webExtBuild(stage, artifactsDir, filename) {
  await mkdir(artifactsDir, { recursive: true });
  await run("npx", [
    "web-ext",
    "build",
    "--source-dir",
    stage,
    "--artifacts-dir",
    artifactsDir,
    "--overwrite-dest",
    "--filename",
    filename,
  ]);
}

async function buildChromium(baseManifest, version) {
  const manifest = structuredClone(baseManifest);
  delete manifest.browser_specific_settings;
  if (manifest.background) {
    manifest.background = {
      service_worker: manifest.background.service_worker,
      type: manifest.background.type,
    };
  }
  const stage = await mkdtemp(path.join(os.tmpdir(), "dload-ext-chrome-"));
  try {
    await stageRuntime(stage, manifest);
    await webExtBuild(stage, path.join(DIST, "chrome"), `send-to-dload-${version}.zip`);
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
}

async function buildFirefox(baseManifest, version) {
  const manifest = structuredClone(baseManifest);
  if (manifest.background) {
    manifest.background = {
      scripts: manifest.background.scripts,
      type: manifest.background.type,
    };
  }
  const zipDir = path.join(DIST, "firefox");
  const stage = await mkdtemp(path.join(os.tmpdir(), "dload-ext-firefox-"));
  try {
    await stageRuntime(stage, manifest);
    await webExtBuild(stage, zipDir, `send-to-dload-${version}.zip`);
    const zipPath = path.join(zipDir, `send-to-dload-${version}.zip`);
    if (existsSync(zipPath)) {
      const xpiPath = path.join(zipDir, `send-to-dload-${version}.xpi`);
      await rm(xpiPath, { force: true });
      await rename(zipPath, xpiPath);
    }
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
}

async function main() {
  const arg = (process.argv[2] || "all").toLowerCase();
  const target = arg === "chrome" ? "chromium" : arg;
  if (!["all", "chromium", "firefox"].includes(target)) {
    throw new Error(`Unknown target '${process.argv[2]}'. Use: chromium | firefox | (none for both).`);
  }

  const baseManifest = await readJSON(path.join(ROOT, "manifest.json"));
  const version = baseManifest.version;
  console.log(`Building send-to-dload v${version} (${target})…`);

  if (target === "all" || target === "chromium") await buildChromium(baseManifest, version);
  if (target === "all" || target === "firefox") await buildFirefox(baseManifest, version);

  console.log("Done. Artifacts in:", DIST);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
