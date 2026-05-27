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

// ─── shell-shim wiring ────────────────────────────────────────────
//
// `install.sh` (curl | bash) appends an `eval "$(flutter-cli init …)"`
// block to the user's shell rc so that `flutter run` is intercepted by
// the TUI. Without this, the binary is on PATH but a bare `flutter
// run` still routes to vanilla flutter — which surprised every user
// who reached for `npm i -g` instead of the curl script.
//
// We mirror install.sh's behavior here, using the same sentinel
// markers so `uninstall.sh` continues to strip both kinds of installs
// in one pass. Everything is wrapped in try/catch and best-effort:
// any failure prints instructions and lets the install succeed,
// because failing the postinstall would break `npm i` of any package
// that transitively depends on us.

const SHIM_MARK_START = '# >>> flutter-cli shim >>>';
const SHIM_MARK_END   = '# <<< flutter-cli shim <<<';

// npm 7+ swallows postinstall stdout/stderr by default — users only
// see "added N packages in Ms" and miss every hint we print. Writing
// to /dev/tty bypasses npm's capture entirely: the message goes
// straight to the controlling terminal. If there's no tty (CI,
// docker non-interactive, piped install), we fall back to stderr.
function notifyUser(message) {
  if (process.platform === 'win32') {
    // No /dev/tty on Windows; stderr works in PowerShell.
    process.stderr.write(message + '\n');
    return;
  }
  try {
    const tty = fs.openSync('/dev/tty', 'w');
    fs.writeSync(tty, message + '\n');
    fs.closeSync(tty);
  } catch {
    process.stderr.write(message + '\n');
  }
}

// Render a hard-to-miss banner. Color codes only kick in when we
// actually have a TTY (no point coloring CI logs).
function banner(title, lines) {
  const useColor = (() => {
    if (process.env.NO_COLOR) return false;
    try { return fs.statSync('/dev/tty').isCharacterDevice(); }
    catch { return false; }
  })();
  const BOLD  = useColor ? '\x1b[1m'  : '';
  const GREEN = useColor ? '\x1b[32m' : '';
  const RESET = useColor ? '\x1b[0m'  : '';
  const bar = '─'.repeat(64);
  const out = [];
  out.push('');
  out.push(`${GREEN}${bar}${RESET}`);
  out.push(`${BOLD}${title}${RESET}`);
  out.push('');
  for (const line of lines) out.push(`  ${line}`);
  out.push(`${GREEN}${bar}${RESET}`);
  return out.join('\n');
}

// Map $SHELL basename → { kind, rc path, eval line }. We use the same
// rc precedence as install.sh.
function detectShell() {
  const sh = process.env.SHELL || '';
  const home = os.homedir();
  if (!home) return null;
  if (sh.endsWith('/zsh'))  return { kind: 'zsh',  rc: path.join(home, '.zshrc'),  evalLine: 'eval "$(flutter-cli init zsh)"' };
  if (sh.endsWith('/bash')) return { kind: 'bash', rc: path.join(home, '.bashrc'), evalLine: 'eval "$(flutter-cli init bash)"' };
  if (sh.endsWith('/fish')) return { kind: 'fish', rc: path.join(home, '.config/fish/config.fish'), evalLine: 'flutter-cli init fish | source' };
  return null;
}

function printManualShimInstructions(reason) {
  // Use the /dev/tty notifyUser path so npm 7+'s default postinstall
  // muting doesn't swallow the instructions — same reasoning as the
  // success banner.
  const sh = detectShell();
  const shellGuess = sh?.kind || 'zsh';
  const rcGuess    = sh?.rc   || '~/.zshrc';
  const evalGuess  = sh?.evalLine || 'eval "$(flutter-cli init zsh)"';
  notifyUser(banner('flutter-cli: shell shim NOT installed', [
    `Reason: ${reason}.`,
    '',
    'Add this line to your shell rc to enable the TUI:',
    '',
    `    ${evalGuess}`,
    '',
    `(typical file for ${shellGuess}: ${rcGuess})`,
    'Then open a new terminal — or `source` the file.',
  ]));
}

function wireShim() {
  // The exec'd binary is what `flutter-cli init <shell>` will resolve
  // to at runtime — but only if it's on PATH. npm puts the wrapper
  // (bin/flutter-cli.js) on PATH automatically, so `flutter-cli` will
  // resolve to the wrapper, which exec's our native binary. That's
  // exactly what we want.

  if (process.env.FLUTTER_CLI_SKIP_SHIM === '1') {
    printManualShimInstructions('FLUTTER_CLI_SKIP_SHIM=1');
    return;
  }
  if (process.env.CI === 'true' || process.env.CI === '1') {
    // CI runs of `npm i -g` are almost never followed by interactive
    // shell work — silently skip so we don't pollute logs.
    return;
  }
  if (process.platform === 'win32') {
    // Windows doesn't have a single rc-file convention that maps to
    // the bash/zsh/fish shim we emit. Users on PowerShell or cmd
    // typically don't need the interception either (they call
    // `flutter-cli` directly). Skip gracefully.
    return;
  }
  // Running under sudo writes into root's HOME — never what the user
  // wants. install.sh has the same guard implicitly (it requires a
  // writable rc and bails on permission errors).
  if (typeof process.getuid === 'function' && process.getuid() === 0) {
    printManualShimInstructions('running as root');
    return;
  }

  const shell = detectShell();
  if (!shell) {
    printManualShimInstructions('could not detect $SHELL');
    return;
  }

  try {
    fs.mkdirSync(path.dirname(shell.rc), { recursive: true });
    // Create the rc if it doesn't exist — install.sh does the same
    // via `touch`, so re-installing on a fresh machine still works.
    if (!fs.existsSync(shell.rc)) {
      fs.writeFileSync(shell.rc, '');
    }
    const current = fs.readFileSync(shell.rc, 'utf8');
    if (current.includes(SHIM_MARK_START)) {
      // Re-install / upgrade path: the shim is already present, so
      // the user's existing terminals continue to work. No banner —
      // they already know how this thing behaves.
      notifyUser(`flutter-cli: shim already present in ${shell.rc} — leaving it as-is`);
      return;
    }
    const block =
      `\n${SHIM_MARK_START}\n` +
      `# Auto-added by @antoinegtir/flutter-cli postinstall. Run \`npm uninstall -g @antoinegtir/flutter-cli\` (or uninstall.sh) to remove.\n` +
      `${shell.evalLine}\n` +
      `${SHIM_MARK_END}\n`;
    fs.appendFileSync(shell.rc, block);
    // npm 7+ swallows postinstall stdout by default — this banner
    // goes to /dev/tty so the user actually sees the "open a new
    // terminal" step. Without this, every fresh `npm i -g` user
    // gets the binary on PATH but stays on vanilla `flutter` until
    // they figure out the missing shell-reload step on their own.
    notifyUser(banner('flutter-cli installed — ONE LAST STEP', [
      `Shim added to ${shell.rc}.`,
      '',
      'The shim is a shell function, so your CURRENT terminal',
      "doesn't know about it yet. To activate the TUI:",
      '',
      '    open a new terminal',
      '',
      'or run:',
      '',
      `    source ${shell.rc}`,
      '',
      'Then `flutter run` opens the multi-device dashboard.',
    ]));
  } catch (err) {
    printManualShimInstructions(`couldn't write ${shell.rc}: ${err.message}`);
  }
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

  // Wire the shim — this is the whole reason `npm i -g` exists as a
  // first-class install path next to `curl | bash`. Best-effort: any
  // failure is logged but doesn't fail the install.
  wireShim();
}

main().catch((err) => {
  console.error(`flutter-cli install failed: ${err.message}`);
  // Do NOT exit non-zero — that would block `npm install` of any
  // package that transitively depends on us. The bin script handles
  // the "binary missing" case at run time with a clearer error.
  process.exit(0);
});
