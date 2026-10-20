import { internalMutation } from './_generated/server'
import { calculateNextRunAt, DEFAULT_TIMEZONE } from './lib/cronUtils'
import {
  MAX_SETTIMEOUT_DELAY_MS,
  RUNNING_TIMEOUT_HEADROOM_MS,
  snapshotRunTimeoutMs,
  STALE_RUN_TIMEOUT_MS,
} from './lib/constants'
import { isPendingOrRunning } from './lib/domain'
import { incrementStat } from './lib/stats'
import { insertRun, patchRun, type RunWriteCtx } from './lib/runStore'

import type { GenericDatabaseWriter } from 'convex/server'
import type { DataModel, Id } from './_generated/dataModel'

/**
 * Schedule Scanner Module
 *
 * Contains internal functions for scanning and processing due schedules.
 * Called by the cron job defined in crons.ts.
 */

interface ScheduleRecord {
  _id: Id<'schedules'>
  jobId: Id<'jobs'>
  cron: string
  timezone?: string
  params?: unknown
  nextRunAt?: number
}

function getTimezone(schedule: ScheduleRecord): string {
  return schedule.timezone ?? DEFAULT_TIMEZONE
}

async function advanceSchedule(
  ctx: { db: GenericDatabaseWriter<DataModel> },
  schedule: ScheduleRecord,
  now: number,
  lastRunAt?: number
): Promise<void> {
  const nextRunAt = calculateNextRunAt(schedule.cron, getTimezone(schedule), new Date(now))
  await ctx.db.patch('schedules', schedule._id, {
    ...(lastRunAt != null && { lastRunAt }),
    nextRunAt,
    updatedAt: now,
  })
}

async function processSchedule(
  ctx: RunWriteCtx,
  schedule: ScheduleRecord,
  now: number
): Promise<{ runCreated: boolean; scheduleUpdated: boolean }> {
  const isDue = !schedule.nextRunAt || schedule.nextRunAt <= now
  if (!isDue) return { runCreated: false, scheduleUpdated: false }

  const job = await ctx.db.get('jobs', schedule.jobId)
  if (!job) {
    console.error(`Job ${schedule.jobId} not found for schedule ${schedule._id}`)
    await ctx.db.patch('schedules', schedule._id, { enabled: false, updatedAt: now })
    return { runCreated: false, scheduleUpdated: true }
  }

  const recentRuns = await ctx.db
    .query('runs')
    .withIndex('by_schedule', q => q.eq('scheduleId', schedule._id))
    .order('desc')
    .take(10)

  if (recentRuns.find(r => isPendingOrRunning(r.status))) {
    await advanceSchedule(ctx, schedule, now)
    return { runCreated: false, scheduleUpdated: true }
  }

  await insertRun(ctx, {
    jobId: schedule.jobId,
    scheduleId: schedule._id,
    status: 'pending',
    triggeredBy: 'schedule',
    input: schedule.params,
    startedAt: now,
  })

  await incrementStat(ctx.db, 'runsTriggered')
  await advanceSchedule(ctx, schedule, now, now)
  return { runCreated: true, scheduleUpdated: true }
}

function accumulateScanResult(
  totals: { runsCreated: number; schedulesUpdated: number },
  result: { runCreated: boolean; scheduleUpdated: boolean }
): void {
  if (result.runCreated) totals.runsCreated++
  if (result.scheduleUpdated) totals.schedulesUpdated++
}

function hasScanChanges(runsCreated: number, schedulesUpdated: number): boolean {
  return runsCreated > 0 || schedulesUpdated > 0
}

/** Bounded number of stale candidates inspected per status, per scan. */
const STALE_RUN_REAP_BATCH_SIZE = 50

interface StaleRunRecord {
  _id: Id<'runs'>
  jobId: Id<'jobs'>
  scheduleId?: Id<'schedules'>
  startedAt: number
  input?: unknown
}

/**
 * Fail a single stuck run and, if it belongs to a schedule, mirror the
 * failure onto the schedule's `lastRunAt`/`lastRunStatus` — the same
 * bookkeeping `runs.fail` performs — so the reaper is indistinguishable
 * from a normal failure everywhere else in the app.
 */
async function reapRun(
  ctx: RunWriteCtx,
  run: StaleRunRecord,
  thresholdMs: number,
  now: number
): Promise<void> {
  await patchRun(ctx, run._id, {
    status: 'failed',
    error: `Run exceeded the stuck-run threshold (${thresholdMs}ms) without completing and was reaped automatically`,
    completedAt: now,
    duration: now - run.startedAt,
  })
  await incrementStat(ctx.db, 'runsFailed')

  if (run.scheduleId) {
    const schedule = await ctx.db.get('schedules', run.scheduleId)
    if (schedule) {
      await ctx.db.patch('schedules', schedule._id, {
        lastRunAt: now,
        lastRunStatus: 'failed',
      })
    }
  }
}

/**
 * The stale-run threshold for a `running` run honors the job's own
 * `config.timeout` plus headroom, when set to a valid positive value, since
 * it can legitimately exceed `STALE_RUN_TIMEOUT_MS`. A configured timeout
 * above `MAX_SETTIMEOUT_DELAY_MS` is ignored (falls back to the flat
 * default) since Node's `setTimeout` cannot actually honor a delay that
 * large.
 * `pending` runs have not started executing yet, so they always use the
 * flat default.
 */
/** Snapshot batches get their account-scaled budget, never less than the job's own. */
function effectiveTimeoutMs(
  config: { command?: string; timeout?: number } | undefined,
  input: unknown
): number | undefined {
  const accounts = (input as { accounts?: unknown } | null | undefined)?.accounts
  if (config?.command !== '__copilot_snapshot__' || !Array.isArray(accounts)) return config?.timeout
  return Math.max(config.timeout ?? 0, snapshotRunTimeoutMs(accounts.length))
}

async function staleRunThresholdMs(
  ctx: { db: GenericDatabaseWriter<DataModel> },
  run: StaleRunRecord,
  status: 'pending' | 'running'
): Promise<number> {
  if (status !== 'running') return STALE_RUN_TIMEOUT_MS

  const job = await ctx.db.get('jobs', run.jobId)
  const configuredTimeout = effectiveTimeoutMs(job?.config, run.input)
  if (
    configuredTimeout != null &&
    configuredTimeout > 0 &&
    configuredTimeout <= MAX_SETTIMEOUT_DELAY_MS
  ) {
    return Math.max(STALE_RUN_TIMEOUT_MS, configuredTimeout + RUNNING_TIMEOUT_HEADROOM_MS)
  }
  return STALE_RUN_TIMEOUT_MS
}

/**
 * Reap runs stuck in `pending` or `running` past their stale-run threshold.
 *
 * Nothing else can clear a run out of `pending`/`running` — `markRunning`,
 * `complete`, `fail`, and `cancel` all require the worker (the Electron IPC
 * layer) to still be alive to call them. If the app is killed or crashes
 * mid-run, the run is stuck forever, and `processSchedule` treats any
 * in-flight run as a reason to skip creating a new one — permanently
 * disabling the schedule (see issue #339). This reaper fails those runs so
 * schedules can dispatch again.
 */
async function reapStaleRuns(ctx: RunWriteCtx, now: number): Promise<number> {
  let reaped = 0

  for (const status of ['pending', 'running'] as const) {
    // react-doctor-disable-next-line react-doctor/async-await-in-loop -- Process two bounded status batches sequentially to limit database work in each reaper transaction.
    const candidates = await ctx.db
      .query('runs')
      .withIndex('by_status', q => q.eq('status', status))
      .order('asc')
      .take(STALE_RUN_REAP_BATCH_SIZE)

    for (const run of candidates) {
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- Reaper walks a bounded batch of candidate rows sequentially per scan.
      const thresholdMs = await staleRunThresholdMs(ctx, run, status)
      if (run.startedAt >= now - thresholdMs) continue
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- Reaper walks a bounded batch of candidate rows sequentially per scan.
      await reapRun(ctx, run, thresholdMs, now)
      reaped++
    }
  }

  return reaped
}

/**
 * Main scan and dispatch function.
 *
 * Called every minute by the cron job. This function:
 * 1. Reaps runs stuck in `pending`/`running` past their stale-run threshold
 *    (`STALE_RUN_TIMEOUT_MS`, extended for `running` runs whose job sets a
 *    longer `config.timeout`), so a dead worker can never permanently block
 *    a schedule (see issue #339)
 * 2. Queries for due schedules
 * 3. Creates pending runs for each due schedule
 * 4. Updates schedule timing (lastRunAt, nextRunAt)
 *
 * Missed occurrences are not caught up: `advanceSchedule` always computes the
 * next run from the current time rather than from the schedule's stored
 * `nextRunAt`, so a schedule that was disabled or unreachable simply resumes
 * from the next occurrence instead of replaying the gap.
 */
export const scanAndDispatch = internalMutation({
  args: {},
  handler: async ctx => {
    const now = Date.now()
    const totals = { runsCreated: 0, schedulesUpdated: 0 }

    const runsReaped = await reapStaleRuns(ctx, now)

    // Get all enabled schedules
    const enabledSchedules = await ctx.db
      .query('schedules')
      .withIndex('by_enabled', q => q.eq('enabled', true))
      .collect()

    for (const schedule of enabledSchedules) {
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- Schedule dispatch mutates run and schedule state sequentially for deterministic scans.
      const result = await processSchedule(ctx, schedule, now)
      accumulateScanResult(totals, result)
    }

    // Log summary for debugging
    if (hasScanChanges(totals.runsCreated, totals.schedulesUpdated) || runsReaped > 0) {
      console.log(
        `Schedule scan complete: ${totals.runsCreated} runs created, ${totals.schedulesUpdated} schedules updated, ${runsReaped} stale runs reaped`
      )
    }

    return {
      runsCreated: totals.runsCreated,
      schedulesUpdated: totals.schedulesUpdated,
      runsReaped,
      scannedAt: now,
    }
  },
})

/**
 * Mark Copilot usage snapshots as due for collection.
 *
 * Called daily by the cron in crons.ts.  Creates a pending snapshot run
 * for each tracked GitHub account, skipping accounts that already have
 * a pending or running run for the current day to avoid duplicates.
 */
export const markSnapshotsDue = internalMutation({
  args: {},
  handler: async ctx => {
    const now = Date.now()
    const todayStart = new Date(now)
    todayStart.setUTCHours(0, 0, 0, 0)
    const todayMs = todayStart.getTime()

    const accounts = await ctx.db.query('githubAccounts').collect()
    if (accounts.length === 0) {
      return { marked: 0 }
    }

    // Find snapshot job by name — create on first run if missing
    let snapshotJob = await ctx.db
      .query('jobs')
      .withIndex('by_name', q => q.eq('name', 'copilot-usage-snapshot'))
      .first()

    if (!snapshotJob) {
      const jobId = await ctx.db.insert('jobs', {
        name: 'copilot-usage-snapshot',
        description: 'Collect immutable Copilot usage/spend snapshots',
        workerType: 'exec',
        config: { command: '__copilot_snapshot__', timeout: 60000 },
        createdAt: now,
        updatedAt: now,
      })
      snapshotJob = await ctx.db.get(jobId)
    }

    if (!snapshotJob) {
      return { marked: 0 }
    }

    // Check for existing pending/running snapshot runs created today
    const recentRuns = await ctx.db
      .query('runs')
      .withIndex('by_job', q => q.eq('jobId', snapshotJob!._id))
      .order('desc')
      .take(50)

    const todayPending = recentRuns.filter(
      r => r.startedAt >= todayMs && isPendingOrRunning(r.status)
    )

    if (todayPending.length > 0) {
      return { marked: 0, reason: 'already pending today' }
    }

    // Create one pending run (input carries the account list for the IPC layer)
    await insertRun(ctx, {
      jobId: snapshotJob._id,
      status: 'pending',
      triggeredBy: 'schedule',
      input: { accounts: accounts.map(a => ({ username: a.username, org: a.org })) },
      startedAt: now,
    })

    await incrementStat(ctx.db, 'runsTriggered')

    return { marked: 1 }
  },
})
