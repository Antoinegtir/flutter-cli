#!/usr/bin/env bash
#
# flutter-cli uninstaller — reverses `install.sh`:
#   1. Strips the `# >>> flutter-cli shim >>>` block from every supported
#      rc file we find (idempotent — skips files where the block isn't
#      present, so it's safe to run twice).
#   2. Removes the `flutter-cli` binary from every known install
#      location (~/.local/bin, ~/.cargo/bin, /usr/local/bin, $BIN_DIR)
#      and `cargo uninstall fl-cli` for legacy build-from-source installs.
#
# Usage:
#   ./uninstall.sh              # remove shim + binary
#   ./uninstall.sh --keep-bin   # remove the shim only, leave the binary
#   ./uninstall.sh --keep-rc    # remove the binary only, leave rc alone
#   BIN_DIR=~/.local/bin ./uninstall.sh   # also check this extra dir

set -euo pipefail

if [ -t 1 ]; then
  BOLD=$'\033[1m'; DIM=$'\033[2m'; GREEN=$'\033[32m'
  YELLOW=$'\033[33m'; RED=$'\033[31m'; RESET=$'\033[0m'
else
  BOLD=""; DIM=""; GREEN=""; YELLOW=""; RED=""; RESET=""
fi
info() { printf "${DIM}·${RESET} %s\n" "$*"; }
ok()   { printf "${GREEN}✓${RESET} %s\n" "$*"; }
warn() { printf "${YELLOW}!${RESET} %s\n" "$*" >&2; }

KEEP_BIN="false"
KEEP_RC="false"
while [ $# -gt 0 ]; do
  case "$1" in
    --keep-bin) KEEP_BIN="true"; shift ;;
    --keep-rc)  KEEP_RC="true"; shift ;;
    -h|--help)
      sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) printf "${RED}✗${RESET} unknown option: %s\n" "$1" >&2; exit 1 ;;
  esac
done

# ─── 1. strip the shim from rc files ───────────────────────────────
if [ "$KEEP_RC" = "false" ]; then
  MARK_START="# >>> flutter-cli shim >>>"
  MARK_END="# <<< flutter-cli shim <<<"
  # Every shell rc / profile file the installer (or a user manually
  # following the README) could have dropped the eval line into. Covers
  # interactive shells (.zshrc/.bashrc), login shells
  # (.zprofile/.bash_profile/.zlogin/.profile), and fish. Each file is
  # checked for the sentinel block before touching, so listing extras
  # has no cost when they don't contain the shim.
  RC_CANDIDATES=(
    "$HOME/.zshrc"
    "$HOME/.zprofile"
    "$HOME/.zlogin"
    "$HOME/.bashrc"
    "$HOME/.bash_profile"
    "$HOME/.profile"
    "$HOME/.config/fish/config.fish"
  )
  for rc in "${RC_CANDIDATES[@]}"; do
    [ -f "$rc" ] || continue
    if ! grep -Fq "$MARK_START" "$rc"; then
      continue
    fi
    # Strip the block in-place. We use a portable awk pass (no GNU
    # sed `-i` extensions) so the script runs on macOS BSD tools too.
    tmp="$(mktemp)"
    awk -v s="$MARK_START" -v e="$MARK_END" '
      $0 == s {in_block=1; next}
      $0 == e {in_block=0; next}
      !in_block {print}
    ' "$rc" > "$tmp"
    # Collapse any trailing blank lines we may have introduced.
    awk 'NF{f=1} f' "$tmp" > "$rc"
    rm -f "$tmp"
    ok "Removed flutter-cli shim from $rc"
  done
fi

# ─── 2. remove the binary ──────────────────────────────────────────
if [ "$KEEP_BIN" = "true" ]; then
  info "Skipped binary removal (--keep-bin)."
else
  removed=0
  # Sweep every dir the installer (current or past) may have used —
  # plus an explicit BIN_DIR override if the user set one on install.
  # Only delete regular files (not symlinks to system services).
  for candidate in \
    ${BIN_DIR:+"$BIN_DIR/flutter-cli"} \
    "$HOME/.local/bin/flutter-cli" \
    "${CARGO_HOME:-$HOME/.cargo}/bin/flutter-cli" \
    "/usr/local/bin/flutter-cli"
  do
    if [ -f "$candidate" ] && [ ! -L "$candidate" ]; then
      rm -f "$candidate"
      ok "Removed $candidate"
      removed=$((removed + 1))
    fi
  done
  # Legacy: tidy cargo's bookkeeping for users who originally did a
  # `cargo install --path crates/fl-cli`, so `cargo install --list`
  # no longer claims fl-cli is installed.
  if command -v cargo >/dev/null && cargo install --list 2>/dev/null | grep -q '^fl-cli '; then
    cargo uninstall fl-cli >/dev/null 2>&1 || true
    ok "Cleaned cargo's record of fl-cli (legacy build-from-source install)"
    removed=$((removed + 1))
  fi
  if [ "$removed" -eq 0 ]; then
    warn "flutter-cli binary not found in any known location — nothing to remove"
  fi
fi

cat <<EOF

${BOLD}Done.${RESET} Open a new terminal (or run ${BOLD}exec \$SHELL${RESET}) to pick up the change.
EOF
