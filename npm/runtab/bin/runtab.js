#!/usr/bin/env node
"use strict";

const { spawnSync } = require("child_process");

const PACKAGES = {
  "linux x64": "runtab-linux-x64",
  "linux arm64": "runtab-linux-arm64",
  "darwin x64": "runtab-darwin-x64",
  "darwin arm64": "runtab-darwin-arm64",
  "win32 x64": "runtab-win32-x64",
};

const key = `${process.platform} ${process.arch}`;
const pkg = PACKAGES[key];
if (!pkg) {
  console.error(`runtab: no prebuilt binary for ${key}.`);
  console.error(`Prebuilt platforms: ${Object.keys(PACKAGES).join(", ")}.`);
  console.error("Build from source instead: https://github.com/root-fr/runtab");
  process.exit(1);
}

const exe = process.platform === "win32" ? "runtab.exe" : "runtab";
let bin;
try {
  bin = require.resolve(`${pkg}/${exe}`);
} catch {
  console.error(`runtab: platform package "${pkg}" is not installed.`);
  console.error(
    "It is an optionalDependency of runtab; reinstall without --no-optional / --omit=optional."
  );
  process.exit(1);
}

const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`runtab: failed to run ${bin}: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status === null ? 1 : result.status);
