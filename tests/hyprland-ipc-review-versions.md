# Review issue 6: suffixed versions

Capability version checks now parse exactly three numeric components, an optional
single `v` prefix and nonempty ASCII prerelease/build identifiers. Future releases
such as `0.58.0-rc1` can pass the version floor and must still pass the Lua/API
probe. `0.57.1-rc1` remains below the required 0.57.1 release; build metadata does
not lower a release. Malformed cores/suffixes and overflow remain invalid replies.

Validation: three capability tests pass, including the actual socket/cache flow
using a suffixed version, malformed input cases and release-floor boundaries.
Warning-denied workspace/all-target clippy and formatting pass. Log:
`/tmp/framework-ipc-review-versions.log`.
