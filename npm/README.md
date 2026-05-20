# `flutter-cli` (npm)

Convenience wrapper around the [flutter-cli][repo] native binary,
distributed via npm so the entire Flutter community (which mostly
already has `node` installed) can install with a single command.

```sh
# One-shot run, no install.
npx flutter-cli run

# Global install (puts `flutter-cli` on your PATH).
npm install -g flutter-cli
```

On install, a small postinstall script downloads the right
prebuilt binary for your platform from the [GitHub Releases][rel]
of the same version. To skip the download (CI, Docker), set
`FLUTTER_CLI_SKIP_DOWNLOAD=1`.

Supported targets:

- macOS arm64 / x86_64
- Linux arm64 / x86_64 (glibc)
- Windows arm64 / x86_64 (msvc)

For anything else, build from source — see the [main repo][repo].

[repo]: https://github.com/Antoinegtir/flutter-cli
[rel]:  https://github.com/Antoinegtir/flutter-cli/releases
