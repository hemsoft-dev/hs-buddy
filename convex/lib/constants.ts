export const MS_PER_DAY = 24 * 60 * 60 * 1000

/**
 * How long a run may remain in `pending` or `running` before the stuck-run
 * reaper in `scheduleScanner.scanAndDispatch` fails it automatically.
 *
 * Justified against the slowest worker default: the ai/skill workers abort
 * their underlying call after 120_000ms (2 minutes), and the exec worker
 * after 30_000ms (see `electron/workers/*Worker.ts`). A run still
 * pending/running an hour after it started has almost certainly lost its
 * worker (e.g. the Electron app was killed or crashed mid-run) rather than
 * genuinely still executing, so this threshold gives ~30x headroom over the
 * slowest built-in default while still freeing a wedged schedule well within
 * the same working day.
 */
export const STALE_RUN_TIMEOUT_MS = 60 * 60 * 1000 // 1 hour

/**
 * Extra headroom added on top of a job's own `config.timeout` (exec-worker
 * only; see `electron/workers/execWorker.ts`) before a `running` run is
 * considered stale. `config.timeout` is unbounded and can legitimately
 * exceed `STALE_RUN_TIMEOUT_MS`, so the reaper must honor it — otherwise a
 * long-running job gets failed mid-execution, a due schedule enqueues a
 * duplicate, and the original worker's later `runs.complete` call
 * overwrites the failure while double-counting both stats.
 */
export const RUNNING_TIMEOUT_HEADROOM_MS = 5 * 60 * 1000 // 5 minutes

/**
 * Node's `setTimeout` delay is a signed 32-bit integer internally; a value
 * above this overflows and fires almost immediately instead of waiting, so
 * `execWorker.ts`'s `setTimeout(() => ..., timeout)` call — where `timeout`
 * is `config.timeout` resolved via `resolveExecConfig` — would not actually
 * honor a `job.config.timeout` configured above this. The reaper
 * excludes such out-of-range values when deriving the stale-run threshold
 * (falling back to `STALE_RUN_TIMEOUT_MS`) so a misconfigured job timeout
 * can't accidentally suppress reaping for a run that is, in practice, no
 * longer running.
 */
export const MAX_SETTIMEOUT_DELAY_MS = 2_147_483_647 // 2^31 - 1

/**
 * Copilot usage snapshot runs (the `__copilot_snapshot__` marker job) collect
 * accounts one after another. The native worker allows 30 seconds plus one
 * minute per captured account, above the marker job's legacy 60-second
 * `config.timeout`; the reaper must not fail a large batch before that.
 */
export function snapshotRunTimeoutMs(accountCount: number): number {
  return 30_000 + 60_000 * accountCount
}

/**
 * Accounts per Copilot usage snapshot run. The native worker accepts at most 128
 * per run, so a larger installation is split across several runs.
 */
export const SNAPSHOT_BATCH_ACCOUNTS = 100
