# Native telemetry and feature-intake qualification

Scope: #823 slice 8, 2026-10-30 EDT. Electron stays primary.

## Export contract

[Electron telemetry](../electron/telemetry.ts) exports OTLP/HTTP protobuf to
`OTEL_EXPORTER_OTLP_ENDPOINT` plus `/v1/traces`, `/v1/metrics`, `/v1/logs`.
Native [buddy-core telemetry](../rust/crates/buddy-core/src/telemetry.rs) uses
that same base endpoint and signal paths, normalizes trailing slashes, batches
traces/logs and exports metrics every 15 seconds. A synchronous quit hook shuts down all three providers before GPUI's 200 ms
async shutdown budget, even if one fails. SDK shutdown waits can add up to fifteen
seconds to exit with an unreachable collector. Forced termination cannot flush.
Export requests time out after two seconds. Authentication headers use the
SDK's `OTEL_EXPORTER_OTLP_HEADERS` and per-signal header settings, as transport
headers only. Redirects are disabled. OS trust roots are used for HTTPS.

No endpoint, an empty endpoint, or an invalid endpoint disables telemetry
silently before creating clients, SDK providers or worker threads. There is no
collector discovery or fallback to localhost. URLs containing user info, query
strings or fragments are rejected. Supply credentials in headers, never URLs.

Resources contain only `service.name` and `service.version`, with no automatic
host/process/environment detectors. Service name accepts `buddy` and
`buddy-native`, falling back to `buddy` for other values; this intentional
allowlist prevents arbitrary environment strings becoming exported attributes.
The version comes from the app's embedded package version. Native deliberately
does not enable verbose exporter diagnostics through `OTEL_LOG_LEVEL`.

Native records startup/window opening and GitHub CLI service operations.
`buddy.ipc.calls`, `buddy.ipc.errors`, and `buddy.ipc.duration` use static
`ipc.channel` labels `app.startup` and `github.cli`. Native has no Electron IPC
transport; these labels describe equivalent native operations. The duration is
milliseconds. `buddy.windows.opened` uses `window.target=primary`. Each operation
ends a span and emits a correlated fixed INFO/WARN log. Failures and cancellation
export a fixed status, never error text. No subprocess arguments/output, tokens,
account IDs, HTTP URLs/headers, prompts or response bodies enter this API.
Unknown attribute keys AND values are omitted. There is no general log bridge
or globally accessible OTel provider through which arbitrary records can escape.

## Aspire launch

The existing [AppHost](../aspire-apphost/apphost.mts) still starts Electron by
default. Set `BUDDY_ASPIRE_NATIVE=1` in the launching shell and run `aspire run`
from the repository. The additional `buddy-native` resource runs `native:dev`
with `.withOtlpExporter()`, using Aspire's injected OTLP endpoint/headers.
Both runtimes can remain open for comparison. No endpoint or header value
should be copied into qualification receipts or logs.

The [Rust OTLP SDK](https://docs.rs/opentelemetry-otlp/0.31.1/opentelemetry_otlp/)
implements HTTP protobuf. The exporter protocol and signal URLs are explicit;
per-signal endpoint/protocol overrides do not redirect native telemetry away
from the configured base. The SDK still supports its header configuration.

## Evidence and remaining qualification

Run from `rust/`:

```sh
cargo test -p buddy-core telemetry::tests
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p buddy-app
```

The [local receiver test](../rust/crates/buddy-core/src/telemetry/tests.rs) decodes
actual protobuf requests for all three signals,
checks metric names and service resources, and proves shutdown flush. Focused
tests cover endpoint paths, disabled state, and omission of secret-like
attributes, including secrets under approved keys. These are local synthetic
receipts, not proof of authenticated production Aspire behavior.

Before closing the integration cutover gate, on each supported OS launch the
opt-in Aspire resource with isolated app data, exercise window creation and a
GitHub request, and confirm the service/version, spans, correlated logs and
15-second metric updates in the dashboard. Confirm failed/cancelled requests,
normal exit flush, unreachable-collector responsiveness, header authentication
and trusted development certificates without bypassing TLS. Repeat with no
endpoint and confirm no collector connections. Retain sanitized screenshots and
collector receipts. No authenticated Aspire/dashboard run is claimed here.

Native does not patch HTTP/DNS globally as Electron's Node instrumentation does.
Other provider operations and secondary browser windows are not instrumented in
this slice. HTTP/DNS traces, Electron channel-for-channel metric coverage and
matching-fixture overhead measurements remain explicit parity work. Existing
provider tests do not prove live authentication or production data equality.

## Feature intake

The source audit found no Electron intake IPC handler, route, screen or caller
of `api.featureIntakes`. The renderer's
[utility file](../src/utils/featureIntakeUtils.ts) only re-exports
[shared pure normalization helpers](../shared/utils/featureIntakeUtils.ts).
The [Convex feature-intake API](../convex/featureIntakes.ts) is for external
clients and remains the same shared backend for both desktop runtimes:

- `normalize` validates input, deduplicates by source/external ID and canonical
  key, and returns canonical issue title/body/labels plus the intake ID.
- Queries list, filter, fetch and find source/external mappings.
- `linkCanonicalIssue` saves a caller-supplied issue number/URL. It does not
  create a GitHub issue or authenticate a GitHub/Jira client.

[Backend tests](../convex/__tests__/featureIntakes.test.ts) and
[utility tests](../src/utils/featureIntakeUtils.test.ts) cover these contracts.
These handlers do not call `ctx.auth`. The tests do not establish deployed
intake access control, GitHub/Jira authentication or external issue creation.

There is no implemented Electron desktop intake workflow to port. Native adds
no new intake UI or credential path. The [vision](VISION.md) still lists a discussion processor. No matching intake
caller or discussion-processor workflow exists in the current source catalog.

Remaining external qualification: against an isolated deployed Convex backend,
submit an intake twice with the same source/external ID, submit a canonical
duplicate, verify normalized fields/labels and mapping, then have the authorized
external issue creator create/link a test issue and verify round-trip queries.
Qualify authorization and failure/retry behavior of that external client
separately. No live external intake, issue creation or production credentials
were exercised for this slice. This documentation resolves the inventory's
ambiguity; it does not claim deployed feature-intake integration is qualified.
