# React Doctor

React Doctor runs as a source health check for TypeScript, React, workflow, and
repository hygiene findings.

Generated output directories are excluded from `doctor.config.json` because they
are build or coverage artifacts, not maintainable source. The gh-aw
`.github/workflows/*.lock.yml` files are also generated; their source files are
the neighboring `.md` workflow definitions. React Doctor's
`build-pipeline-secret-boundary` rule is ignored only for those generated lock
files so findings must be fixed in workflow sources or in hand-maintained YAML.

Run `bun install --frozen-lockfile`, then `bun run react-doctor`. Windows and
Ubuntu use React Doctor 0.9.22 from the exact development dependency and
`bun.lock`, with the same `doctor.config.json`. Node runs the CLI; Bun runs the
repository wrapper. No external score service is required. The PowerShell
`scripts/run-react-doctor.ps1 -ScoreOnly` command delegates to this full scan.
The dependency override also aligns `react-scan`'s React Doctor dependency with
the direct pin, avoiding a second analyzer version.

The wrapper uses `--no-score`: the maintained gate requires a complete scan with
zero unsuppressed errors and warnings. It reports the native score as unavailable
when `summary.score` is null; clean diagnostics do not imply a numeric score.
A native score is displayed only when supplied by the analyzer, separately from
the diagnostic gate. The legacy `-ScoreOnly` switch still runs this full offline
scan and does not enable the score service. See the [React Doctor CLI reference](https://www.react.doctor/docs/reference/cli-reference).

The command writes `reports/react-doctor/report.json` and `stderr.txt`, including
the analyzer version, rule, file path, and line number. It fails on every
unsuppressed error or warning, incomplete scans, skipped checks, malformed
reports, and unexpected schema versions. There is no accepted diagnostic count
to increase. CI runs both operating systems, preserves reports even on failure,
and requires the matrix job in `ci-complete`.

## Updating and suppressing findings

Update with `bun add --dev --exact react-doctor@<version>` and commit both
`package.json` and `bun.lock`. Run the full command, inspect every diagnostic,
and verify the parser against the new JSON schema. Both CI platforms must pass.
Fix findings before enforcement or link separately tracked work in the PR.
Do not introduce a numeric baseline or globally disable a new rule to pass CI.

For a verified false positive or intentional behavior, use a
`react-doctor-disable-next-line` comment naming one rule and explaining the
specific invariant. Place it immediately before the reported line. File-scoped
overrides need an exact path and a rationale here. Existing global ignores are
legacy policy; this upgrade adds no new global rule exclusions.

The 0.9.13 baseline had 23 warnings and zero errors. Five collection operations
were simplified. The remaining findings were triaged as follows:

| Location | Exception rationale |
| --- | --- |
| `convex/lib/runStore.ts` | Stop readiness queries at the first unmigrated job before querying aggregates. |
| `convex/scheduleScanner.ts` | Two bounded status batches run sequentially to limit transaction work. |
| `electron/ipc/githubHandlers.ts` | Sequential probe batches enforce the existing concurrency cap. |
| `electron/preload.ts` | Each asserted map entry is initialized or checked synchronously before access. |
| `src/services/dataCache.ts` | Clear gates must be rechecked after each await because a replacement clear can start. |
| `src/hooks/useCopilotSessions.ts`, `src/hooks/useTodoist.ts` | Loading resets already occur in `finally`, guarded against unmounted updates. |
| `src/components/PullRequestHistoryPanel.tsx` | The loading reset is in `finally` and restricted to the latest request. Private rendering components stay beside their only caller; an exact-file override covers `no-multi-component-file`. |
| `src/components/settings/SettingsNotifications.tsx` | The URL is held in a ref and revoked by the shared callback on completion, failure, replacement, and unmount. |

The 0.9.22 update added no suppressions. Its new findings were fixed in code:
`useGitHubData` and `useOrgCachedFetch` now reset state during render when the
cache key changes instead of in a layout effect, and `useUsageProviderRetry`
moves retry-timer arming from its event handler into `armRetry`.

To verify regression detection, temporarily add a component that assigns
`ref.current` during render under `src/`, run `bun run react-doctor`, and confirm
a nonzero exit with the rule, path, and line. Remove the probe and rerun to
restore the zero-diagnostic result. Gate/parser tests cover warnings, errors,
malformed reports, skipped scans, and the CI dependency wiring.
