#!/usr/bin/env node
// Thin shim that exec's the native binary downloaded by install.js.
// Keeps stdio fully inherited so the TUI renders correctly.

const path = require('node:path');
const fs = require('node:fs');
const { spawn } = require('node:child_process');

const isWin = process.platform === 'win32';
const binName = isWin ? 'flutter-cli.exe' : 'flutter-cli-bin';
const bin = path.join(__dirname, binName);

if (!fs.existsSync(bin)) {
  console.error(
    `flutter-cli: native binary not found at ${bin}.\n` +
    `If this is a fresh install, the postinstall download may have failed —\n` +
    `re-run \`npm install -g flutter-cli\` or download a binary directly from\n` +
    `https://github.com/Antoinegtir/flutter-cli/releases`
  );
  process.exit(1);
}

const child = spawn(bin, process.argv.slice(2), { stdio: 'inherit' });
child.on('exit', (code, signal) => {
  if (signal) {
    // Re-raise the signal so callers see the same exit semantics as
    // running the native binary directly.
    process.kill(process.pid, signal);
  } else {
    process.exit(code ?? 1);
  }
});
