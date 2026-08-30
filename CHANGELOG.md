# Changelog

## Unreleased

- Reused established fuzzy matching and edit-distance implementations in the search service.
- Reduced JSONL and output actor branching while preserving ordered output behavior.
- Consolidated related core identity and envelope modules to improve locality.
- Removed output cloning, repeated D-Bus watch logic, wildcard imports, and test panic paths.
- Added a reproducible Rust Quality Lens configuration.

## 0.1.0

- Added the `shelllist-daemon-core` runtime-independent crate.
- Added the `shelllist-daemon-tokio` D-Bus, JSONL, ownership, shutdown, and subscription crate.
- Moved `shelllist-search` into the workspace without changing its binary or JSONL contract.
- Added versioned API/event envelope builders, protocol fixture helpers, monotonic IDs, and secure atomic JSON state helpers.
- Added a shared ordered JSONL client runner with configurable correlation, cancellation, and call-failure policies.
