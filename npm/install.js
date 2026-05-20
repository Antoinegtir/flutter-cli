#!/usr/bin/env node
// Postinstall hook for the `flutter-cli` npm package.
//
// Resolves the current package version, maps Node's process.platform /
// process.arch to one of our GitHub Release archive names, downloads
// that archive, extracts the binary into ./bin/, and chmod +x's it.
// This is the same pattern esbuild / swc / biome use for binary-backed
// CLIs distributed via npm.

const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const https = require('node:https');
const { execSync } = require('node:child_process');
const zlib = require('node:zlib');
const { pipeline } = require('node:stream/promises');

const pkg = require('./package.json');
// We decouple the npm wrapper version from the binary version: the
// wrapper can bump for README / packaging fixes without us having to
// cut a fresh GitHub Release of the same binary. `binaryVersion` in
// package.json wins; otherwise we fall back to pkg.version.
const VERSION = pkg.binaryVersion || pkg.version;
const REPO = 'Antoinegtir/flutter-cli';

// Map { platform, arch } → release-asset suffix + archive format.
const TARGETS = {
  'darwin-arm64':  { target: 'aarch64-apple-darwin',       archive: 'tar.gz' },
  'darwin-x64':    { target: 'x86_64-apple-darwin',        archive: 'tar.gz' },
  'linux-arm64':   { target: 'aarch64-unknown-linux-gnu',  archive: 'tar.gz' },
  'linux-x64':     { target: 'x86_64-unknown-linux-gnu',   archive: 'tar.gz' },
  'win32-arm64':   { target: 'aarch64-pc-windows-msvc',    archive: 'zip'    },
  'win32-x64':     { target: 'x86_64-pc-windows-msvc',     archive: 'zip'    },
};

function platformKey() {
  return `${process.platform}-${process.arch}`;
}

function pickTarget() {
  const key = platformKey();
  const hit = TARGETS[key];
  if (!hit) {
    const supported = Object.keys(TARGETS).join(', ');
    throw new Error(
      `flutter-cli: no prebuilt binary for ${key}. ` +
      `Supported: ${supported}. ` +
      `Build from source: https://github.com/${REPO}#manual-install`
    );
  }
  return hit;
}

function assetUrl({ target, archive }) {
  const name = `flutter-cli-${VERSION}-${target}.${archive}`;
  return `https://github.com/${REPO}/releases/download/v${VERSION}/${name}`;
}

function download(url) {
  return new Promise((resolve, reject) => {
    const req = https.get(url, (res) => {
      if (res.statusCode === 301 || res.statusCode === 302) {
        // GitHub release assets redirect to a CDN — follow once.
        return resolve(download(res.headers.location));
      }
      if (res.statusCode !== 200) {
        return reject(new Error(`GET ${url} returned ${res.statusCode}`));
      }
      resolve(res);
    });
    req.on('error', reject);
  });
}

async function extractTarGz(stream, destDir) {
  // Use the host's `tar` — bundled with macOS, Linux, and every
  // recent Windows. Cheaper than pulling a JS tar implementation.
  const tmp = path.join(os.tmpdir(), `flutter-cli-${Date.now()}.tar.gz`);
  await pipeline(stream, fs.createWriteStream(tmp));
  fs.mkdirSync(destDir, { recursive: true });
  execSync(`tar -xzf "${tmp}" -C "${destDir}"`);
  fs.unlinkSync(tmp);
}

async function extractZip(stream, destDir) {
  // Same idea, but with `tar -xf` (handles zip on modern Windows) or
  // PowerShell's Expand-Archive as fallback.
  const tmp = path.join(os.tmpdir(), `flutter-cli-${Date.now()}.zip`);
  await pipeline(stream, fs.createWriteStream(tmp));
  fs.mkdirSync(destDir, { recursive: true });
  try {
    execSync(`tar -xf "${tmp}" -C "${destDir}"`);
  } catch {
    // Fallback for older Windows without bsdtar.
    execSync(
      `powershell -NoProfile -Command "Expand-Archive -Force -Path '${tmp}' -DestinationPath '${destDir}'"`
    );
  }
  fs.unlinkSync(tmp);
}

async function main() {
  // Allow CI / docker builds to skip the download (binaries are
  // typically vendored elsewhere). Same pattern as esbuild.
  if (process.env.FLUTTER_CLI_SKIP_DOWNLOAD === '1') {
    console.log('flutter-cli: FLUTTER_CLI_SKIP_DOWNLOAD=1, skipping');
    return;
  }

  const target = pickTarget();
  const url = assetUrl(target);
  const binDir = path.join(__dirname, 'bin');
  fs.mkdirSync(binDir, { recursive: true });

  console.log(`flutter-cli: downloading ${url}`);
  const stream = await download(url);

  const stageDir = path.join(os.tmpdir(), `flutter-cli-stage-${Date.now()}`);
  fs.mkdirSync(stageDir, { recursive: true });

  if (target.archive === 'tar.gz') {
    await extractTarGz(stream, stageDir);
  } else {
    await extractZip(stream, stageDir);
  }

  // The archive expands to `flutter-cli-<version>-<target>/<binary>`.
  // Pluck the binary and drop it into ./bin/.
  const inner = path.join(
    stageDir,
    `flutter-cli-${VERSION}-${target.target}`
  );
  const binName = process.platform === 'win32' ? 'flutter-cli.exe' : 'flutter-cli';
  const src = path.join(inner, binName);
  const dst = path.join(binDir, binName === 'flutter-cli.exe' ? 'flutter-cli.exe' : 'flutter-cli-bin');
  fs.copyFileSync(src, dst);
  if (process.platform !== 'win32') {
    fs.chmodSync(dst, 0o755);
  }
  fs.rmSync(stageDir, { recursive: true, force: true });
  console.log(`flutter-cli: installed ${dst}`);
}

main().catch((err) => {
  console.error(`flutter-cli install failed: ${err.message}`);
  // Do NOT exit non-zero — that would block `npm install` of any
  // package that transitively depends on us. The bin script handles
  // the "binary missing" case at run time with a clearer error.
  process.exit(0);
});
