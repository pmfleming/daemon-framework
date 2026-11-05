# Hyprland IPC compatibility contract

Phase 1 targets Hyprland 0.56.2 and 0.57.1. The coordinated desktop upgrade must
land the framework and all path-dependent consumers together. Later phases may
raise the supported floor only after the user confirms that upgrade.

- Workspace numbers are optional. Special workspaces in 0.57 have names and
  addresses, but no numeric id. An absent id never becomes workspace zero in
  ordinary-workspace guards, window placement, allocation or history.
- Window workspace references preserve the number, address and name when
  available, and identify special workspaces across both supported shapes.
- Geometry resolves special workspace rules and client occupancy by name when a
  number is unavailable. Empty special-workspace monitor entries are absent;
  two missing numbers never establish identity by themselves.
- Command replies are bounded to 16 MiB and event lines to 64 KiB, including the
  line terminator. Limits apply to input bytes before lossy UTF-8 decoding.
  Invalid text becomes U+FFFD; invalid JSON remains an explicit decoding error.
  Invalid text alone never tears down the event stream.
- The monitor argument to a pinned batch snapshot must satisfy the monitor
  module's connector-identifier validator. Lua string quoting alone does not
  protect the IPC batch delimiter. Rejection happens before connecting or
  creating any weak window references.
- Existing exact-session mutation routing, atomic Lua acknowledgement and
  window/process/workspace guards remain in force. Special workspaces cannot
  satisfy an ordinary-workspace guard.

Evidence: the captured fixtures under `crates/shelllist-hyprland/tests/fixtures`,
workspace and geometry unit tests, bounded socket tests, and the opt-in
`private_workspace` Wayland client acceptance test. The Phase 0 host setup and
results are recorded in `tests/hyprland-ipc-baseline.md`.
