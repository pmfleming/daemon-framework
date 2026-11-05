#!/usr/bin/env bash
set -euo pipefail

if (( $# > 1 )); then
  printf 'usage: %s [framework-source-root]\n' "$0" >&2
  exit 2
fi
acceptance_root=${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}
: "${WORKSPACE_TEST_HYPRLAND:=/run/current-system/sw/bin/Hyprland}"
: "${WORKSPACE_TEST_WESTON:?Nix wiring supplies the Weston test applications}"
if [[ -z "${WORKSPACE_TEST_PARENT_DISPLAY:-}" && -n "${WAYLAND_DISPLAY:-}" ]]; then
  if [[ "$WAYLAND_DISPLAY" = /* ]]; then
    export WORKSPACE_TEST_PARENT_DISPLAY="$WAYLAND_DISPLAY"
  else
    export WORKSPACE_TEST_PARENT_DISPLAY="${XDG_RUNTIME_DIR:?}/$WAYLAND_DISPLAY"
  fi
fi
: "${WORKSPACE_TEST_PARENT_DISPLAY:?A Wayland renderer/socket is required for private acceptance}"
test -x "$WORKSPACE_TEST_HYPRLAND"
test -x "$WORKSPACE_TEST_WESTON/bin/weston-flower"
test -S "$WORKSPACE_TEST_PARENT_DISPLAY"
export WORKSPACE_TEST_HYPRLAND WORKSPACE_TEST_WESTON
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/shelllist-hyprland-acceptance}"
mkdir -p "$CARGO_TARGET_DIR"
cd -- "$acceptance_root"
exec cargo test --locked -p shelllist-hyprland --test private_workspace -- --ignored --nocapture
