# Contributing to flutter-cli

Thanks for being here. flutter-cli only gets better with people poking
at it on devices and OSes the maintainer doesn't own, so even a "tried
it on my Pixel 9, here's what broke" issue is genuinely useful.

## Quick start

```sh
git clone https://github.com/Antoinegtir/flutter-cli
cd flutter-cli

# Build the binary.
cargo build --release --bin flutter-cli

# Run the unit tests + the headless integration tests
# (the latter only run on Unix; Windows skips them automatically).
cargo test --workspace --locked
```

Rust toolchain is pinned in `rust-toolchain.toml` — `rustup` will pick
up the right version on first build.

## Trying your change locally

```sh
# Without the shell shim — call the binary directly.
./target/release/flutter-cli run

# Or wire the shim against your dev binary so `flutter run` triggers it.
eval "$(./target/release/flutter-cli init bash)"   # or zsh / fish
flutter run
```

## Before opening a PR

CI runs three checks; running them locally first saves a round-trip:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

PR template walks you through the rest (title, screenshots for TUI
changes, etc.). Keep changes small — a 200-line PR gets reviewed; a
2000-line PR gets a "let's split this up first" comment and stalls.

## Where to start

- Issues labelled [`good first issue`][gfi] are scoped tight enough
  for a first PR (~1-2 hours of work, no architectural decisions).
- Issues labelled [`help wanted`][hw] are bigger — usually new
  features or platform support that the maintainer can't easily test
  alone.
- If you want to propose something not yet on the roadmap, open a
  [feature request][fr] first so we can agree on the shape before you
  write code.

[gfi]: https://github.com/Antoinegtir/flutter-cli/issues?q=is%3Aopen+is%3Aissue+label%3A%22good+first+issue%22
[hw]:  https://github.com/Antoinegtir/flutter-cli/issues?q=is%3Aopen+is%3Aissue+label%3A%22help+wanted%22
[fr]:  https://github.com/Antoinegtir/flutter-cli/issues/new?template=feature.yml

## Reporting a bug

Use the [bug report form][br] — it asks for the four things that
actually matter (repro, version, OS, shell). A 30-second screen
recording is worth a thousand words of "the TUI looks weird".

[br]: https://github.com/Antoinegtir/flutter-cli/issues/new?template=bug.yml

## Project layout

```
crates/
  fl-core/          # Shared types: events, config, errors
  fl-adb/           # Android device discovery, ADB wrapper, Wi-Fi pairing
  fl-ios/           # iOS via xcrun devicectl / idevice tooling
  fl-flutter/       # Flutter daemon parser + spawn
  fl-vmservice/     # Dart VM service WebSocket client
  fl-tui/           # ratatui dashboard, views, panels
  fl-cli/           # Binary entry point (clap CLI, init shim)
.github/workflows/  # CI (lint + matrix test) and tag-driven release
```

When fixing a bug, prefer adding a test in the closest crate (parser
tests in `fl-flutter/src/parse.rs`, daemon tests in `fl-flutter/src/daemon.rs`,
TUI render tests in `fl-tui/`, integration tests in `crates/fl-cli/tests/`).

## Code style

We default to whatever `cargo fmt` + `cargo clippy -D warnings` say.
Comments explain *why*, not *what* — assume the reader can read Rust.

## Security

For anything sensitive (RCE, credential leak, sandbox escape) please
use the [private vulnerability reporting flow](SECURITY.md) instead of
a public issue.

## License

By contributing, you agree your work is released under the MIT
license, same as the rest of the project.
