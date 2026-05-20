# Security policy

## Supported versions

`flutter-cli` ships a single supported version at any time — whatever the
latest GitHub Release is. Older versions don't receive backported fixes;
upgrade to the latest binary or rebuild from `master`.

| Version            | Supported          |
| ------------------ | ------------------ |
| latest release     | :white_check_mark: |
| anything older     | :x:                |

## Reporting a vulnerability

**Please do not open a public issue for security reports.**

Instead, use GitHub's private vulnerability reporting:

1. Go to the [Security tab](https://github.com/Antoinegtir/flutter-cli/security/advisories/new).
2. Fill in the form with as much detail as you can — affected versions,
   reproduction steps, suggested remediation if you have one.

You should get an acknowledgement within **72 hours**. If you don't,
ping [@Antoinegtir](https://github.com/Antoinegtir) on the issue tracker
asking them to check their Security tab (without disclosing the issue
contents).

## Scope

In scope:

- `flutter-cli` itself — anything in this repo, including the install
  script and the shell shim.
- The way it spawns / interacts with the `flutter` and `adb` / `xcrun`
  binaries.

Out of scope:

- Vulnerabilities in the upstream `flutter` SDK, `adb`, `xcrun`,
  `libimobiledevice`, or any other third-party tool we shell out to —
  report those to their respective maintainers.
- Vulnerabilities in user code running inside a Flutter app under
  `flutter-cli`. The TUI does not modify the app's behaviour beyond
  what `flutter run` would do.

## Triage and fix policy

- **Critical** (remote code execution, credential leak): patched within
  7 days, advisory published within 14 days.
- **High** (local privilege escalation, denial-of-service): patched in
  the next release, advisory published with the release.
- **Medium / Low**: addressed in regular release cadence; usually no
  separate advisory.

## Disclosure

Once a fix is shipped, we publish a GitHub Security Advisory crediting
the reporter (unless you've asked to stay anonymous). No CVE is filed
automatically — request one from us if you'd like it tracked publicly.
