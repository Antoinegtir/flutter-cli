# `flutter-cli` — `flutter run`, but with superpowers

[![npm version](https://img.shields.io/npm/v/@antoinegtir/flutter-cli.svg?logo=npm)](https://www.npmjs.com/package/@antoinegtir/flutter-cli)
[![npm downloads](https://img.shields.io/npm/dm/@antoinegtir/flutter-cli.svg?logo=npm)](https://www.npmjs.com/package/@antoinegtir/flutter-cli)
[![CI](https://github.com/Antoinegtir/flutter-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/Antoinegtir/flutter-cli/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/Antoinegtir/flutter-cli/blob/master/LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/Antoinegtir/flutter-cli?style=social)](https://github.com/Antoinegtir/flutter-cli)

A modern terminal UI for Flutter — **hot reload across N devices**, **live HTTP / perf inspector**, inline scrollback. Drops into your shell so `flutter run` *becomes* the dashboard. No new command to learn.

![flutter-cli landing](https://github.com/Antoinegtir/flutter-cli/raw/master/docs/screenshots/landing.gif)

## Install

```sh
# Zero install — just run it.
npx @antoinegtir/flutter-cli run

# Or global.
npm install -g @antoinegtir/flutter-cli
flutter-cli run
```

On install, the right prebuilt binary for your platform is fetched from [GitHub Releases][rel]. Set `FLUTTER_CLI_SKIP_DOWNLOAD=1` to skip it (useful in CI / Docker layers).

| Supported | |
|---|---|
| macOS | arm64 + x86_64 |
| Linux | arm64 + x86_64 (glibc) |
| Windows | arm64 + x86_64 (msvc) |

## Why

`flutter run` was written for one device, one developer, one terminal. In 2025 you're probably:

- Testing on **2+ devices simultaneously** (iOS, Android, simulator).
- Watching FPS, memory, jank ratios — not just compile errors.
- Drowning in 50,000 lines of scrollback per session.
- Re-typing the same `flutter run --device emulator-5554 --flavor prod` for the hundredth time.

Same project, same `flutter` binary underneath, dramatically better feedback loop.

|  | vanilla `flutter run` | with `flutter-cli` |
|---|---|---|
| Multi-device hot reload | one at a time | parallel, single `r` |
| Per-device FPS / memory | no | yes, live sparklines |
| Inline TUI (scrollback preserved) | no | yes |
| Live HTTP inspector | DevTools-only | `n` keystroke, in your terminal |
| Side-by-side screenshots | per-platform tooling | `s` keystroke, all devices |
| Open DevTools | copy URL manually | `d` keystroke |
| Skip the TUI when you need | n/a | `--basic` flag |

## Keys at a glance

`r` reload · `R` restart · `b` brightness · `p` debug paint · `P` perf overlay · `o` platform · `s` screenshot · `n` network · `d` DevTools · `/` filter · `c` copy · `q` quit

## Want the shell shim?

Want `flutter run` (not `flutter-cli run`) to fire the TUI directly? Add this **one line** to your shell config:

```sh
# bash / zsh
eval "$(flutter-cli init bash)"   # or zsh
# fish
flutter-cli init fish | source
```

After reloading your shell, the literal command `flutter run` is intercepted by the TUI. `flutter pub`, `flutter doctor`, `flutter clean` and everything we don't enhance pass through to the real `flutter` binary unchanged. **Your IDE keeps using vanilla `flutter`** — the shim only fires in your terminal.

## Full docs

Screenshots, every key binding, the multi-device picker, the network inspector, integration test workflows, the `--basic` passthrough flag, contributing guide and roadmap all live in the main repo.

➡️ **[github.com/Antoinegtir/flutter-cli][repo]**

## License

MIT.

[repo]: https://github.com/Antoinegtir/flutter-cli
[rel]:  https://github.com/Antoinegtir/flutter-cli/releases
