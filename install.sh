#!/usr/bin/env bash
#
# flutter-cli installer — downloads the pre-built `flutter-cli` binary
# for your platform from the latest GitHub Release, verifies its
# sha256, drops it on your PATH, and wires the shell shim into your
# rc file so `flutter run` / `flutter test` / `flutter build` /
# `flutter devices` route through the TUI automatically.
#
# Idempotent: re-running upgrades the binary and skips the rc edit
# if the eval line is already there.
#
# Designed for the canonical `curl ... | bash` one-liner — no clone,
# no Rust toolchain required. Running `./install.sh` from a clone
# works the same way.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Antoinegtir/flutter-cli/master/install.sh | bash
#   ./install.sh                       # detect everything
#   ./install.sh --shell zsh           # force the rc file we patch
#   ./install.sh --no-shim             # binary only, skip the rc edit
#   ./install.sh --version 0.4.0       # pin a specific release
#   BIN_DIR=~/.local/bin ./install.sh  # override install dir
#
# Exit codes: 0=success, 1=user/env error, 2=download/verify failure.

set -euo pipefail

# ─── pretty logging ────────────────────────────────────────────────
if [ -t 1 ]; then
  BOLD=$'\033[1m'; DIM=$'\033[2m'; GREEN=$'\033[32m'
  YELLOW=$'\033[33m'; RED=$'\033[31m'; RESET=$'\033[0m'
else
  BOLD=""; DIM=""; GREEN=""; YELLOW=""; RED=""; RESET=""
fi
info()  { printf "${DIM}·${RESET} %s\n" "$*"; }
ok()    { printf "${GREEN}✓${RESET} %s\n" "$*"; }
warn()  { printf "${YELLOW}!${RESET} %s\n" "$*" >&2; }
fail()  { printf "${RED}✗${RESET} %s\n" "$*" >&2; exit "${2:-1}"; }

REPO="Antoinegtir/flutter-cli"

# ─── arg parsing ───────────────────────────────────────────────────
FORCE_SHELL=""
INSTALL_SHIM="true"
FL_VERSION="${FL_VERSION:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    --shell)    FORCE_SHELL="$2"; shift 2 ;;
    --no-shim)  INSTALL_SHIM="false"; shift ;;
    --version)  FL_VERSION="$2"; shift 2 ;;
    -h|--help)
      # Print the top header block as help when invoked from a file.
      # When piped via `curl | bash` there's no file to read; fall
      # back to a one-liner so --help never crashes the installer.
      if [ -f "$0" ]; then
        sed -n '2,27p' "$0" | sed 's/^# \{0,1\}//'
      else
        echo "flutter-cli installer — see https://github.com/$REPO"
      fi
      exit 0 ;;
    *) fail "unknown option: $1 (try --help)" ;;
  esac
done

# ─── 1. detect platform target ─────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS-$ARCH" in
  Darwin-arm64)              TARGET=aarch64-apple-darwin      ;;
  Darwin-x86_64)             TARGET=x86_64-apple-darwin       ;;
  Linux-aarch64|Linux-arm64) TARGET=aarch64-unknown-linux-gnu ;;
  Linux-x86_64)              TARGET=x86_64-unknown-linux-gnu  ;;
  MINGW*|MSYS*|CYGWIN*-*)
    fail "Windows is not supported by this script — install via npm:
    npm i -g @antoinegtir/flutter-cli" ;;
  *)
    fail "unsupported platform $OS-$ARCH — install via npm instead:
    npm i -g @antoinegtir/flutter-cli" ;;
esac
info "Platform: $OS / $ARCH → $TARGET"

# ─── 2. required tools ─────────────────────────────────────────────
for tool in curl tar awk; do
  command -v "$tool" >/dev/null || fail "$tool not found — please install it first"
done
SHACMD=""
if   command -v shasum    >/dev/null 2>&1; then SHACMD="shasum -a 256"
elif command -v sha256sum >/dev/null 2>&1; then SHACMD="sha256sum"
else fail "neither shasum nor sha256sum is available — cannot verify download"
fi

# ─── 3. resolve version ────────────────────────────────────────────
if [ -z "$FL_VERSION" ]; then
  info "Resolving latest release from GitHub..."
  # /releases/latest returns JSON; extract tag_name without needing jq.
  # `head -1` keeps us tolerant of preview/draft fields appearing later.
  FL_VERSION="$(
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
      | sed -nE 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v?([^"]+)".*/\1/p' \
      | head -1
  )"
  [ -n "$FL_VERSION" ] \
    || fail "could not resolve the latest version (GitHub API rate-limited? set FL_VERSION=X.Y.Z to bypass)"
fi
FL_VERSION="${FL_VERSION#v}"
info "Installing flutter-cli v$FL_VERSION"

# ─── 4. download tarball + checksum ────────────────────────────────
ASSET="flutter-cli-${FL_VERSION}-${TARGET}.tar.gz"
BASE="https://github.com/${REPO}/releases/download/v${FL_VERSION}"

TMP="$(mktemp -d 2>/dev/null || mktemp -d -t flutter-cli)"
trap 'rm -rf "$TMP"' EXIT

info "Downloading $ASSET..."
curl -fsSL --retry 3 -o "$TMP/$ASSET"        "$BASE/$ASSET"        || fail "download failed: $BASE/$ASSET" 2
curl -fsSL --retry 3 -o "$TMP/$ASSET.sha256" "$BASE/$ASSET.sha256" || fail "checksum download failed: $BASE/$ASSET.sha256" 2

info "Verifying sha256..."
expected="$(awk '{print $1; exit}' "$TMP/$ASSET.sha256")"
actual="$( $SHACMD "$TMP/$ASSET" | awk '{print $1}' )"
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
  printf "expected: %s\nactual:   %s\n" "$expected" "$actual" >&2
  fail "sha256 mismatch — refusing to install a tampered binary" 2
fi
ok "sha256 verified"

# ─── 5. extract + install ──────────────────────────────────────────
tar -xzf "$TMP/$ASSET" -C "$TMP"
SRC_BIN="$TMP/flutter-cli-${FL_VERSION}-${TARGET}/flutter-cli"
[ -x "$SRC_BIN" ] || fail "expected binary not found at $SRC_BIN" 2

# Pick install dir: explicit env wins; else first one of ~/.local/bin
# (XDG) or ~/.cargo/bin that already exists; else create ~/.local/bin.
BIN_DIR="${BIN_DIR:-${FL_PREFIX:+$FL_PREFIX/bin}}"
if [ -z "$BIN_DIR" ]; then
  if   [ -d "$HOME/.local/bin" ]; then BIN_DIR="$HOME/.local/bin"
  elif [ -d "$HOME/.cargo/bin" ]; then BIN_DIR="$HOME/.cargo/bin"
  else                                 BIN_DIR="$HOME/.local/bin"
  fi
fi
mkdir -p "$BIN_DIR"
install -m 0755 "$SRC_BIN" "$BIN_DIR/flutter-cli"
BIN="$BIN_DIR/flutter-cli"
ok "flutter-cli v$FL_VERSION installed at $BIN"

# Sanity-check PATH so we don't claim success and leave the user
# unable to invoke the binary.
case ":${PATH:-}:" in
  *":$BIN_DIR:"*) ;;
  *) warn "$BIN_DIR is not on your PATH — add this to your rc file:
    export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac

# ─── 6. wire the shell shim ────────────────────────────────────────
if [ "$INSTALL_SHIM" = "false" ]; then
  ok "Skipped shell shim (--no-shim). Done."
  exit 0
fi

# Decide which rc file to patch. Explicit > $SHELL > zsh fallback.
shell_name() {
  if [ -n "$FORCE_SHELL" ]; then
    echo "$FORCE_SHELL"; return
  fi
  case "${SHELL:-}" in
    */zsh)  echo zsh ;;
    */bash) echo bash ;;
    */fish) echo fish ;;
    *)      echo zsh ;;  # macOS default since Catalina
  esac
}

SHELL_KIND="$(shell_name)"
case "$SHELL_KIND" in
  zsh)  RC="$HOME/.zshrc" ;;
  bash) RC="$HOME/.bashrc" ;;
  fish) RC="$HOME/.config/fish/config.fish" ;;
  *) fail "unsupported shell: $SHELL_KIND (try --shell zsh|bash|fish)" ;;
esac
mkdir -p "$(dirname "$RC")"
touch "$RC"

# Markers let `uninstall.sh` (and re-runs of this script) find our
# block to update / remove without disturbing other rc content.
MARK_START="# >>> flutter-cli shim >>>"
MARK_END="# <<< flutter-cli shim <<<"

if grep -Fq "$MARK_START" "$RC"; then
  info "flutter-cli shim already present in $RC — leaving it as-is"
else
  info "Patching $RC with the flutter-cli shim..."
  if [ "$SHELL_KIND" = "fish" ]; then
    EVAL_LINE='flutter-cli init fish | source'
  else
    EVAL_LINE="eval \"\$(flutter-cli init $SHELL_KIND)\""
  fi
  {
    printf "\n%s\n" "$MARK_START"
    printf "# Auto-added by flutter-cli install.sh. Run uninstall.sh to remove.\n"
    printf "%s\n" "$EVAL_LINE"
    printf "%s\n" "$MARK_END"
  } >> "$RC"
  ok "Added flutter-cli shim to $RC"
fi

# ─── 7. final summary ──────────────────────────────────────────────
cat <<EOF

${BOLD}Done.${RESET} To activate the shim in your current shell:

    ${BOLD}source $RC${RESET}

Then try ${BOLD}flutter run${RESET} — the TUI should take over.
Other ${BOLD}flutter ...${RESET} commands (pub, doctor, clean, ...) pass through unchanged.
EOF
