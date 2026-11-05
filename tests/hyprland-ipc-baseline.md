# Hyprland IPC gap closure: Phase 0

Recorded 2026-11-05 on branch `hyprland-ipc-gaps`. Main checkouts were clean.
Baseline heads: framework `5d2d51e`, app `eec44e1`, bar `361b96f`,
Shelllist `1a7d5ed3`, updater `7c4b393`.

Both delayed update timers were confirmed inactive before implementation.
`update-worker status` reported queued=true, ready=false, staged=false.
No service restart, rebuild, activation or reboot was performed.

## Baseline checks

- Framework, in `nix develop`: fmt, workspace/all-target clippy with warnings
  denied, locked workspace tests: passed (77 passed, 2 opt-in tests ignored).
- App: `nix develop --no-write-lock-file --command cargo test --locked`:
  passed (62 unit tests and 19 integration tests; opt-in tests ignored).
- Bar: same command: passed (323 unit tests, 33 integration tests; opt-in tests
  ignored). The notification concurrency test took most of the 140s unit run.
- Shelllist portal launcher: 4 passed. Existing lockfile lacked the framework's
  `rustix` edge; `--locked` initially refused. Plain `cargo test` repaired only
  that missing dependency edge, with no package version changes.
- Installed Lua configuration with 0.57.1 `Hyprland --verify-config --config
  ~/.config/hypr/hyprland.lua`, and
  `LUA_PATH="$HOME/.config/hypr/?.lua;;"`: **config ok**. An isolated runtime
  directory was used. The verification-only process also logs an unused socket2
  bind error; configuration verification succeeds.
- Existing `private_workspace` acceptance test: passed on both installed 0.56.2
  and queued-revision 0.57.1 (1 test each). Private IPC, config, applications and
  runtime directories; existing-parent mode used the running compositor only as
  Wayland host. All children exited. Weston supplied `weston-flower` through Nix.

## Private compositor evidence

`crates/shelllist-hyprland/tests/fixtures/{0.56.2,0.57.1}/` contains captured
monitors, workspaces, clients, activewindow, workspace rules and outer gaps,
with a mapped Weston flower before and after moving it to `special:ipc-probe`.
Titles and monitor descriptions/identifiers were redacted. `events.txt` is a
passive socket2 recording from the same private instance, with titles redacted.

Confirmed live on 0.57.1: the special workspace and its window's workspace have
`address="special:ipc-probe"`, `type="special"`, and **no id**. On 0.56.2 they
have id -98. Special v2 events replace that numeric id with the address.
Source inspection of v0.57.1 also confirms `changeworkspaceid` (old id,new id).
A real invalid-UTF-8 title remains unverified; this does not block Phase 1.

The default Weston 15.0.1 host fails because Aquamarine requests protocol versions
it does not implement (wl_compositor v6). Weston 16.0.0 has the same limitation;
headless Sway 1.12 fails at xdg_wm_base v6. Weston 12 required its own Mesa loader
and drivers and then failed at wl_compositor v6. These are test-host failures,
not IPC acceptance failures. The existing test's `WORKSPACE_TEST_PARENT_DISPLAY`
option succeeds. Reproduce with:

```sh
WORKSPACE_TEST_PARENT_DISPLAY="$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" \
WORKSPACE_TEST_WESTON=/nix/store/jkp1gi6393c85x0dfibhglwp46lmzcaa-weston-15.0.1 \
WORKSPACE_TEST_HYPRLAND=/run/current-system/sw/bin/Hyprland \
nix develop --command cargo test --locked -p shelllist-hyprland \
  --test private_workspace -- --ignored --nocapture
```

For 0.57.1, use the compositor built/substituted from
`github:NixOS/nixpkgs/1d060086d60cb76a40cf37109f979d88a5bd6396#hyprland`:
`/nix/store/c7zh46h1xflvd56iqvw672l8gfl6jc85-hyprland-0.57.1/bin/Hyprland`.

## Phase 1S preflight

Quickshell v0.3.2 **builds successfully** from its own flake with nixpkgs following
Shelllist's approved `8eeec934ae0dbeca3d7868c059568a65c08b2fc3`.
Result: `/nix/store/32rvs0v76yqgfx10lqdp6ad8ajw1cyqc-quickshell-wrapped-0.3.2`.
The approved Qt remains the sole Qt supplier. This preflight did not edit the
Shelllist or system flakes.

| Component | Installed display pin `6774f7bc2` | Queued pin `1d060086d` |
| --- | --- | --- |
| Hyprland | 0.56.2 | 0.57.1 |
| Kernel | 6.18.53 | 6.18.60 |
| Mesa | 26.2.3 | 26.2.6 |
| Aquamarine | 0.15.1 | 0.16.0 |
| Hyprland portal | 1.4.1 | 1.5.0 |

Quickshell changes from approved 0.3.0 to 0.3.2. Its workspace `id` is -1 for
non-numbered workspaces on 0.57; consumers needing a destination must use
`address`. Phase 1S must review the popup activation context accordingly.
