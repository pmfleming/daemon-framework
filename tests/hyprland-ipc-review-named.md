# Named-workspace review closure

The private installed Hyprland 0.57.1 acceptance passed on 2026-11-05, evidence
root `/tmp/ws-hjldmG`. New redacted `fixtures/0.57.1/named.json` and
`named-events.txt` were captured there, not invented from assumptions. Both move
forms carry a bare named address; JSON omits its numeric ID and reports `normal`.

Workspace and monitor reads retain WorkspaceRef identity. Literal selectors use
`name:<address>` so reserved names and renamed displays cannot redirect a launch.
Lua quoting safely accommodates spaces/punctuation in named destinations. The
obsolete negative-ID special heuristic is removed from the shared identity.
Numbered pin guards still reject named targets in the real private compositor.

Additional finding: geometry's `n[true]` rule still used pre-0.57 negative IDs.
It now means a normal workspace without a number, matching 0.57.1
`workspace/filter/statement/NamedStatement.cpp`. Geometry compares available
addresses before display names, avoiding collisions between renamed workspaces.
The new regression distinguishes named, numbered and special rule matching.

Validation: full locked framework tests passed, then all 33 Hyprland unit tests
and warning-denied workspace/all-target clippy passed after the geometry addition.
Private acceptance passed with named read, event, selector and pin-guard checks.
Formatting passes. Logs: `/tmp/framework-ipc-review-{named,named-geometry,private}.log`.
