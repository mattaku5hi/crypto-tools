# trading-dashboard (initial status slice)

Read-only Rust HTML rendering module shared by separately hosted trading systems. `Snapshot` contains an explicit health state and optional evidence; unknown values render `UNKNOWN`, never zero. User-supplied text is HTML-escaped. `render_status_panel` embeds in an existing authenticated page; `render_document` produces a standalone read-only page. Styles are scoped via `.td-panel`.

**This module owns no listener, cookie, credentials, API routes, wallet, orders, forecasts, refresh logic or P&L.** Each host must authenticate and bound/redact its data *before* rendering, and choose a separate port/session. It must not label a fixture as a healthy predictor. Rendering a status panel in two hosts validates only their display seam, not operational 24/7 market coverage, safety or economics.

Current scope is a small common status view, not a replacement for Polydoghound's existing single-page monitor. Grow the shared module only from actual host needs; retain host-specific sections rather than publishing their domain model here. No package-registry release or execution controls are authorized by this source repository.

Run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`. Tests verify unknown states and escaping without network access. License status: `UNLICENSED` pending an owner decision.
