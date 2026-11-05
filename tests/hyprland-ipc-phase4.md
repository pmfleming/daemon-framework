# Hyprland IPC gap closure: Phase 4

`workspace::Window` now supplies title, focus rank, floating/fullscreen state and
Hash to app/bar/portal consumers. Workspace display fields default, while
required window address, PID and workspace and monitor identity remain required.
Addresses remain strings in snapshots; consumers filter invalid selectors per
window instead of failing a complete otherwise-valid snapshot. Optional workspace
numbers retain special identity and are never used for ordinary allocation.

Both JSON deserialization and Rust `Window::default` preserve unfocused rank
`i64::MAX` and mapped=true. Window therefore uses a manual Default implementation
rather than deriving numeric zero for focus rank. Other common models derive
Default, and literals can use struct-update defaults when fields are added.

`Client::status` uses one workspaces/monitors/activewindow batch. `Status::parse`
rejects missing identity and trailing data, and treats `{}`/null active-window
objects as absent. `Client::active_workspace` supplies the portal's typed read.
Malformed typed reads retain an InvalidReply classification. Reads remain
independent of Lua mutation capability checks.

The bar projects this status into its existing wire models. Its active-window
wire field retains zero as the existing non-numbered sentinel; ordinary workspace
selection filters absent/nonpositive IDs before projection. Workspace JSON has
no urgent field, and an audit found no other adapter producer of urgency. The
public wire field therefore stays false, matching stock live behavior; a test
fixture that invented an urgent workspace key was corrected. The richer
display-policy monitor model remains separate because advertised modes, connector
eligibility and topology safety are outside the shared status model.

App preserves Hash-based revisions, ranking, mapped filtering, individual address
filtering and required identity. Old test fixtures that omitted workspace identity
were made explicit; close observation tests still assert only captured PID/address
targets and distinguish an unavailable snapshot from closed windows. Snapshot
parser captures now target only 0.57.1 under D5; 0.56.2 captures remain historical.

Validation: framework fmt, warning-denied workspace/all-target clippy and locked
workspace suite; app/bar full suites and portal tests. The Hyprland crate has
25 tests including a single-command status fixture, empty active-window behavior,
display/default fields, invalid individual addresses, missing identity and
trailing-data rejection. App adds a regression covering ranking and filtering
with the shared model. Raw logs: `/tmp/hyprland-phase4-{framework-final,
app-final,bar,portal}.log`. No live service restart or system activation occurred.
