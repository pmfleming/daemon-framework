# Review issue 4: production capability scope

The only special-toggle caller was private acceptance. The builder and explicit
API check now live there, rather than making an unused API mandatory for every
production mutation. The builder-coverage test still checks every production
Lua API. Private acceptance still constructs and executes the special toggle.

Validation: all 33 Hyprland unit tests and the private installed 0.57.1 test pass;
private evidence root `/tmp/ws-6bCICj`, log
`/tmp/framework-ipc-review-capabilities.log`. Formatting passes.
