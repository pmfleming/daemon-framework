# Changelog

## 0.1.0

- Added the `shelllist-daemon-core` runtime-independent crate.
- Added the `shelllist-daemon-tokio` D-Bus, JSONL, ownership, shutdown, and subscription crate.
- Moved `shelllist-search` into the workspace without changing its binary or JSONL contract.
- Added versioned API/event envelope builders, protocol fixture helpers, monotonic IDs, and secure atomic JSON state helpers.
- Added a shared ordered JSONL client runner with configurable correlation, cancellation, and call-failure policies.
