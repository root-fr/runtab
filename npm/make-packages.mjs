// Assembles publishable npm packages into npm/dist/ from prebuilt binaries.
//
//   node npm/make-packages.mjs --version 0.1.0 --artifacts <dir> [--allow-missing]
//
// <dir> must contain one subdirectory per platform package (runtab-linux-x64,
// runtab-darwin-arm64, ...), each holding the binary (runtab or runtab.exe).
// Missing platforms abort unless --allow-missing is given, in which case the
// main package's optionalDependencies only reference the platforms present.
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync, chmodSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const NPM_DIR = dirname(fileURLToPath(import.meta.url));

const PLATFORMS = [
  { pkg: "runtab-linux-x64", os: "linux", cpu: "x64" },
  { pkg: "runtab-linux-arm64", os: "linux", cpu: "arm64" },
  { pkg: "runtab-darwin-x64", os: "darwin", cpu: "x64" },
  { pkg: "runtab-darwin-arm64", os: "darwin", cpu: "arm64" },
  { pkg: "runtab-win32-x64", os: "win32", cpu: "x64" },
];

const args = process.argv.slice(2);
function flag(name) {
  const i = args.indexOf(name);
  return i === -1 ? null : args[i + 1];
}
const version = flag("--version");
const artifacts = flag("--artifacts");
const allowMissing = args.includes("--allow-missing");

if (!version || !artifacts) {
  console.error("usage: node npm/make-packages.mjs --version <x.y.z> --artifacts <dir> [--allow-missing]");
  process.exit(1);
}
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`invalid version: ${version}`);
  process.exit(1);
}

const dist = join(NPM_DIR, "dist");
rmSync(dist, { recursive: true, force: true });

const present = [];
for (const p of PLATFORMS) {
  const exe = p.os === "win32" ? "runtab.exe" : "runtab";
  const src = join(artifacts, p.pkg, exe);
  if (!existsSync(src)) {
    if (allowMissing) {
      console.error(`skipping ${p.pkg}: ${src} not found`);
      continue;
    }
    console.error(`missing binary for ${p.pkg}: ${src}`);
    process.exit(1);
  }
  const out = join(dist, p.pkg);
  mkdirSync(out, { recursive: true });
  cpSync(src, join(out, exe));
  if (p.os !== "win32") chmodSync(join(out, exe), 0o755);
  writeFileSync(
    join(out, "package.json"),
    JSON.stringify(
      {
        name: p.pkg,
        version,
        description: `runtab binary for ${p.os} ${p.cpu}`,
        repository: { type: "git", url: "git+https://github.com/root-fr/runtab.git" },
        license: "MIT",
        preferUnplugged: true,
        os: [p.os],
        cpu: [p.cpu],
        files: [exe],
      },
      null,
      2
    ) + "\n"
  );
  present.push(p.pkg);
}

if (present.length === 0) {
  console.error("no platform binaries found");
  process.exit(1);
}

const mainOut = join(dist, "runtab");
cpSync(join(NPM_DIR, "runtab"), mainOut, { recursive: true });
const mainPkg = JSON.parse(readFileSync(join(mainOut, "package.json"), "utf8"));
mainPkg.version = version;
mainPkg.optionalDependencies = Object.fromEntries(present.map((pkg) => [pkg, version]));
writeFileSync(join(mainOut, "package.json"), JSON.stringify(mainPkg, null, 2) + "\n");

console.log(`assembled ${present.length + 1} packages in ${dist}:`);
for (const pkg of [...present, "runtab"]) console.log(`  ${pkg}@${version}`);
