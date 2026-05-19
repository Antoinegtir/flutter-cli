#!/usr/bin/env bash
#
# fl uninstaller — reverses `install.sh`:
#   1. Strips the `# >>> fl shim >>>` block from every supported rc
#      file we find (idempotent — skips files where the block isn't
#      present, so it's safe to run twice).
#   2. Removes the `fl` binary via `cargo uninstall`.
#
# Usage:
#   ./uninstall.sh              # remove shim + binary
#   ./uninstall.sh --keep-bin   # remove the shim only, leave fl in place
#   ./uninstall.sh --keep-rc    # remove the binary only, leave rc alone

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
  MARK_START="# >>> fl shim >>>"
  MARK_END="# <<< fl shim <<<"
  RC_CANDIDATES=(
    "$HOME/.zshrc"
    "$HOME/.bashrc"
    "$HOME/.bash_profile"
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
    ok "Removed fl shim from $rc"
  done
fi

# ─── 2. remove the binary ──────────────────────────────────────────
if [ "$KEEP_BIN" = "true" ]; then
  info "Skipped binary removal (--keep-bin)."
else
  if command -v cargo >/dev/null && cargo install --list 2>/dev/null | grep -q '^fl-cli '; then
    cargo uninstall fl-cli >/dev/null
    ok "Uninstalled fl binary (cargo uninstall fl-cli)"
  else
    # Best-effort fallback if the user installed manually or via a
    # different toolchain. Look for fl in the usual cargo bin dirs and
    # the standard system paths, but only remove things that look like
    # ours (regular file, executable, not a system service).
    for candidate in \
      "${CARGO_HOME:-$HOME/.cargo}/bin/fl" \
      "$HOME/.local/bin/fl" \
      "/usr/local/bin/fl"
    do
      if [ -x "$candidate" ] && [ ! -L "$candidate" ]; then
        rm -f "$candidate"
        ok "Removed $candidate"
      fi
    done
  fi
fi

cat <<EOF

${BOLD}Done.${RESET} Open a new terminal (or run ${BOLD}exec \$SHELL${RESET}) to pick up the change.
EOF
