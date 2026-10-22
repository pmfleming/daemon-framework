# Method-proxy cost investigation (2026-10-22)

No production proxy cache is introduced: the proposed match-rule round trips
were not observed, and controlled release timings do not justify extra lifetime
state.

`dbus::tests::method_only_proxies_add_no_match_round_trips_and_follow_owner_replacement`
uses a private real dbus-daemon and the standard BecomeMonitor interface (the same
monitoring mechanism used by busctl). Ten `JsonDbusClient::call` requests, each
constructing a fresh zbus proxy, produced **10 Call, 0 AddMatch/RemoveMatch, and
0 GetNameOwner/GetAll** messages from that client before the final request.
The test also replaces the well-known service owner and verifies that the same
client reaches the replacement. No desktop bus/service is touched.

This agrees with zbus 5.18's implementation: property caching is lazy, and
method-only calls do not install the owner/signal matches used by subscriptions.
This finding does not mean signal subscriptions or owner monitoring are free.

The ignored `profile_fresh_and_reused_method_proxies` probe alternates 1,000 calls
per side, three times, on one private bus. Both sides perform identical JSON
parameter encoding and response decoding. Setup and the cached proxy's initial
construction are outside the timers. Release results in milliseconds:

| Run | Fresh proxy/call | Reused proxy |
| --- | ---: | ---: |
| 1 | 48.584 | 49.180 |
| 2 | 47.642 | 47.638 |
| 3 | 51.391 | 50.914 |

The differences are small and inconsistent in sign. This is not evidence of an
end-to-end Shelllist latency improvement, nor a reason to change owner/cancellation
semantics. JSON-inside-D-Bus and bridge JSONL framing remain unchanged; changing
that public protocol needs separate workload and compatibility evidence.

Reproduce in the development environment:

```
cargo test --release --locked -p shelllist-daemon-tokio dbus::tests
cargo test --release --locked -p shelllist-daemon-tokio profile_fresh_and_reused_method_proxies -- --ignored --nocapture
```
