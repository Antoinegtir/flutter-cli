#!/usr/bin/env bash
#
# fl installer — builds the `fl` binary from source via `cargo install`
# and wires the shell shim into your rc file so `flutter run` /
# `flutter test` / `flutter build` / `flutter devices` route through
# the `fl` TUI automatically. Idempotent: re-running upgrades the
# binary and skips the rc edit if the eval line is already there.
#
# Usage:
#   ./install.sh                  # detect everything
#   ./install.sh --shell zsh      # force the rc file we patch
#   ./install.sh --no-shim        # build the binary only, skip the rc edit
#   FL_PREFIX=~/.local ./install.sh   # cargo install --root <prefix>
#
# Exit codes follow the convention 0=success, 1=user/env error, 2=build
# failure. Errors are explicit ("rust not found" → install command);
# anything we can detect we tell you how to fix.

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
fail()  { printf "${RED}✗${RESET} %s\n" "$*" >&2; exit 1; }

# ─── arg parsing ───────────────────────────────────────────────────
FORCE_SHELL=""
INSTALL_SHIM="true"
while [ $# -gt 0 ]; do
  case "$1" in
    --shell)    FORCE_SHELL="$2"; shift 2 ;;
    --no-shim)  INSTALL_SHIM="false"; shift ;;
    -h|--help)
      sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) fail "unknown option: $1 (try --help)" ;;
  esac
done

# ─── 1. build the binary ───────────────────────────────────────────
command -v cargo >/dev/null \
  || fail "cargo not found. Install Rust first: https://rustup.rs (one-liner: curl https://sh.rustup.rs -sSf | sh)"

# Resolve the workspace root: the directory this script lives in.
# Lets the user run `./install.sh` from anywhere.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

PREFIX="${FL_PREFIX:-}"
info "Building fl (release profile)…"
if [ -n "$PREFIX" ]; then
  cargo install --path crates/fl-cli --force --root "$PREFIX"
  BIN="$PREFIX/bin/fl"
else
  cargo install --path crates/fl-cli --force
  BIN="$(cargo install --path crates/fl-cli --force 2>&1 | awk '/Replacing/ {print $2}' | tr -d '`')"
  # Fallback when the heuristic above fails (e.g. on first install
  # cargo prints "Installing" instead of "Replacing").
  [ -x "$BIN" ] || BIN="${CARGO_HOME:-$HOME/.cargo}/bin/fl"
fi
ok "fl installed at $BIN"

# Sanity-check PATH so we don't claim success and leave the user
# unable to invoke the binary.
case ":$PATH:" in
  *":$(dirname "$BIN"):"*) ;;
  *) warn "$(dirname "$BIN") is not on your PATH — add this to your rc file:
    export PATH=\"$(dirname "$BIN"):\$PATH\"" ;;
esac

# ─── 2. wire the shell shim ────────────────────────────────────────
if [ "$INSTALL_SHIM" = "false" ]; then
  ok "Skipped shell shim (--no-shim). Done."
  exit 0
fi

# Decide which rc file to patch. Explicit > $SHELL > zsh fallback.
shell_name() {
  if [ -n "$FORCE_SHELL" ]; then
    echo "$FORCE_SHELL"
    return
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
MARK_START="# >>> fl shim >>>"
MARK_END="# <<< fl shim <<<"

if grep -Fq "$MARK_START" "$RC"; then
  info "fl shim already present in $RC — leaving it as-is"
else
  info "Patching $RC with the fl shim…"
  if [ "$SHELL_KIND" = "fish" ]; then
    EVAL_LINE='fl init fish | source'
  else
    EVAL_LINE="eval \"\$(fl init $SHELL_KIND)\""
  fi
  {
    printf "\n%s\n" "$MARK_START"
    printf "# Auto-added by fl install.sh. Run uninstall.sh to remove.\n"
    printf "%s\n" "$EVAL_LINE"
    printf "%s\n" "$MARK_END"
  } >> "$RC"
  ok "Added fl shim to $RC"
fi

# ─── 3. final summary ──────────────────────────────────────────────
cat <<EOF

${BOLD}Done.${RESET} To activate the shim in your current shell:

    ${BOLD}source $RC${RESET}

Then try ${BOLD}flutter run${RESET} — the fl TUI should take over.
Other ${BOLD}flutter ...${RESET} commands (pub, doctor, clean, …) pass through unchanged.
EOF
