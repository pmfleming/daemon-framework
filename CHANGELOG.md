# Changelog

## Unreleased

- Share operation/subscription response correlation through `CorrelationPolicy::operation_id`, and add `OwnedOperations::drain` for clone-free bulk claims.
- Remove unused legacy `OwnedTaskRegistry::cancel_all`/`cancel_owner` and `wait_for_owner_loss`/`wait_for_owner_name_loss` APIs after auditing all local consumers. Use managed owner lifetimes, `cancel_owned`, `shutdown`, or an explicit connection-scoped `OwnerLossMonitor::wait`. This is a source API removal; deployed D-Bus/JSONL contracts are unchanged.

- Consolidated `shelllist-hyprland` into a separate workspace crate, preserving its API and tests. App/bar daemons share it through the framework; no standalone checkout or vendored copy is needed.

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
