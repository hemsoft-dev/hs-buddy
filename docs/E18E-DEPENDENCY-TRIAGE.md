# e18e dependency triage

`bun run e18e` is a direct-dependency gate. It must fail for package metadata
errors or new duplicate warnings rooted in this package, but it should not block
on broad transitive duplication that this app cannot safely flatten directly.

## Qualification requires a completed analysis

The analyzer is an exact development-dependency pin in `package.json` and
`bun.lock`. Both analysis and migration use that installed version, without
fetching a floating CLI at runtime. `bun run e18e` prints the installed analyzer
version and rejects a version that differs from the manifest pin.

A missing executable, nonzero exit, signal, five-minute timeout, or output-buffer
failure fails the command. Empty or malformed JSON also fails, as does an
incomplete report. Validation requires this checkout's package name and version,
nonnegative integer dependency counts, a unique duplicate-count statistic, and
well-formed messages using the pinned analyzer's severity and score fields.
Only a successful process with a valid report can qualify dependencies.

This does not make transitive warnings blocking. A valid report containing only
transitive warnings or the documented direct exceptions below still passes.
Metadata errors and undocumented direct duplicate findings still fail.
CI's `Analyze dependency health` step is blocking; it has no
`continue-on-error` override.

The wrapper creates an Electron entry-point placeholder only when no real bundle
exists and removes its own placeholder even on failure. It preserves an existing
bundle. Regression tests cover report validation, process failures and timeout,
warning policy, version drift, and placeholder cleanup.

## Current direct exceptions

These root packages intentionally remain different from some transitive
versions reported by e18e:

- `@types/node`: the app targets Node 22, so the root type package stays on the
  Node 22 line even though Electron and other tooling bring Node 24 types.
- `@opentelemetry/resources` and `@opentelemetry/sdk-metrics`: the app keeps
  these OpenTelemetry packages aligned for its direct instrumentation surface
  while upstream telemetry toolchains may carry older compatible releases
  transitively.
- `es-module-lexer`: the root and `import-in-the-middle` use 3.x, while Vitest 5
  requires `^2.3.2`. Keep both supported major versions rather than overriding
  Vitest's declared range.
- `esbuild`: the app keeps a root `esbuild` for Vite/react-scan peer coverage,
  while Convex currently pins its own older `esbuild` release.
- `globals`: the app keeps the root lint environment catalog current for ESLint
  configuration while markdown and test tooling may carry older releases.
- `prettier`: the app keeps the root formatter current for repository format
  checks while markdown tooling may carry an older formatter release.
- `puppeteer-core`: the Electron memory sampler uses 25.11.0. Lighthouse 13.4.1
  still resolves 25.10.0 transitively, within its declared `^25.3.0` range.
  Let Lighthouse keep its tested copy instead of overriding its dependency tree.
- `typescript`: the app uses TypeScript 6, while `eslint-plugin-sonarjs` still
  carries a TypeScript 5 transitive dependency.
- `vscode-jsonrpc`: the app uses v9 directly, while `@github/copilot-sdk`
  currently carries v8 transitively.

## Current transitive-only warning families

The remaining duplicate warnings are owned by upstream toolchains such as
Electron, Vite/Rolldown, ESLint, Lighthouse, markdown tooling, and test tooling.
They should be reduced through safe upstream package updates, not root-level
overrides that can create incompatible package trees.

## Maintenance rule

When e18e reports a new duplicate warning containing `root@`, either update the
direct dependency to eliminate it or add a short justification above. Do not
silence package metadata errors.
