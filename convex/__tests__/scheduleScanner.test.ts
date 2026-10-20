import aggregateComponent from '@convex-dev/aggregate/test'
import { convexTest as createConvexTest } from 'convex-test'
import { describe, test, expect } from 'vitest'
import schema from '../schema'
import { api, internal } from '../_generated/api'
import {
  MAX_SETTIMEOUT_DELAY_MS,
  RUNNING_TIMEOUT_HEADROOM_MS,
  STALE_RUN_TIMEOUT_MS,
} from '../lib/constants'

const modules = import.meta.glob('../**/*.*s')

function convexTest(schemaDefinition: typeof schema, moduleFiles: typeof modules) {
  const t = createConvexTest(schemaDefinition, moduleFiles)
  aggregateComponent.register(t, 'runCounts')
  return t
}

const baseJob = {
  name: 'scan-job',
  workerType: 'exec' as const,
  config: { command: 'run' },
}

describe('scheduleScanner.scanAndDispatch', () => {
  test('returns zeros when no enabled schedules exist', async () => {
    const t = convexTest(schema, modules)
    const result = await t.mutation(internal.scheduleScanner.scanAndDispatch)
    expect(result.runsCreated).toBe(0)
    expect(result.schedulesUpdated).toBe(0)
    expect(result.scannedAt).toBeGreaterThan(0)
  })

  test('skips schedules that are not yet due', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)

    // Create a schedule with nextRunAt far in the future
    const schedId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'future-sched',
      cron: '0 9 * * *',
      enabled: true,
      missedPolicy: 'skip',
    })

    // Manually push nextRunAt to the future to ensure it won't fire
    const futureMs = Date.now() + 10 * 60 * 60 * 1000 // 10 hours
    await t.mutation(api.schedules.advanceNextRun, { id: schedId, nextRunAt: futureMs })

    const result = await t.mutation(internal.scheduleScanner.scanAndDispatch)
    expect(result.runsCreated).toBe(0)
  })

  test('creates run for a due schedule and advances nextRunAt', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)

    // Create a schedule that is due (nextRunAt in the past)
    const schedId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'due-sched',
      cron: '* * * * *',
      enabled: true,
      missedPolicy: 'skip',
    })

    // Push nextRunAt to the past
    const pastMs = Date.now() - 60000
    await t.mutation(api.schedules.advanceNextRun, { id: schedId, nextRunAt: pastMs })

    const result = await t.mutation(internal.scheduleScanner.scanAndDispatch)
    expect(result.runsCreated).toBe(1)
    expect(result.schedulesUpdated).toBe(1)

    // Verify a run was created
    const runs = await t.query(api.runs.listByStatus, { status: 'pending' })
    expect(runs).toHaveLength(1)
    expect(runs[0].triggeredBy).toBe('schedule')
  })

  test('skips a due schedule that already has a pending/running run', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)

    const schedId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'busy-sched',
      cron: '* * * * *',
      enabled: true,
      missedPolicy: 'skip',
    })

    // Push nextRunAt to the past
    const pastMs = Date.now() - 60000
    await t.mutation(api.schedules.advanceNextRun, { id: schedId, nextRunAt: pastMs })

    // Create a pending run for this schedule
    await t.mutation(api.runs.create, { jobId, scheduleId: schedId, triggeredBy: 'schedule' })

    const result = await t.mutation(internal.scheduleScanner.scanAndDispatch)
    expect(result.runsCreated).toBe(0)
    expect(result.schedulesUpdated).toBe(1) // schedule still updated with new nextRunAt
  })

  test('disables schedule and skips run creation when job is deleted', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)

    const schedId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'orphan-sched',
      cron: '* * * * *',
      enabled: true,
      missedPolicy: 'skip',
    })

    // Push nextRunAt to the past so the schedule is due
    const pastMs = Date.now() - 60000
    await t.mutation(api.schedules.advanceNextRun, { id: schedId, nextRunAt: pastMs })

    // Delete only the job (not the schedule) to simulate an orphaned schedule.
    // We bypass jobs.remove (which cascade-deletes schedules) by using t.run
    // for direct DB access.
    await t.run(async ctx => {
      await ctx.db.delete(jobId)
    })

    // Schedule still exists, but its job is gone
    const schedules = await t.query(api.schedules.list)
    expect(schedules).toHaveLength(1)

    // Scanner should disable the orphaned schedule and not create a run
    const result = await t.mutation(internal.scheduleScanner.scanAndDispatch)
    expect(result.runsCreated).toBe(0)
    expect(result.schedulesUpdated).toBe(1)

    // Verify the schedule was disabled
    const updated = await t.query(api.schedules.get, { id: schedId })
    expect(updated?.enabled).toBe(false)
  })

  test('increments runsTriggered stat for each run created', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)

    const schedId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'stat-sched',
      cron: '* * * * *',
      enabled: true,
      missedPolicy: 'skip',
    })

    const pastMs = Date.now() - 60000
    await t.mutation(api.schedules.advanceNextRun, { id: schedId, nextRunAt: pastMs })

    await t.mutation(internal.scheduleScanner.scanAndDispatch)

    const stats = await t.query(api.buddyStats.get)
    expect(stats.runsTriggered).toBeGreaterThanOrEqual(1)
  })
})

describe('scheduleScanner.scanAndDispatch — stuck-run reaper', () => {
  test('reaps a manual run stuck in pending past the threshold', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const runId = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })

    // Simulate a run abandoned well past the stuck-run threshold
    await t.run(async ctx => {
      await ctx.db.patch(runId, { startedAt: Date.now() - STALE_RUN_TIMEOUT_MS - 60_000 })
    })

    const result = await t.mutation(internal.scheduleScanner.scanAndDispatch)
    expect(result.runsReaped).toBe(1)

    const run = await t.query(api.runs.get, { id: runId })
    expect(run?.status).toBe('failed')
    expect(run?.error).toMatch(/stuck-run threshold/i)
    expect(run?.completedAt).toBeGreaterThan(0)
    expect(run?.duration).toBeGreaterThan(0)
  })

  test('reaps a run stuck in running past the threshold', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const runId = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.markRunning, { id: runId })

    await t.run(async ctx => {
      await ctx.db.patch(runId, { startedAt: Date.now() - STALE_RUN_TIMEOUT_MS - 60_000 })
    })

    const result = await t.mutation(internal.scheduleScanner.scanAndDispatch)
    expect(result.runsReaped).toBe(1)

    const run = await t.query(api.runs.get, { id: runId })
    expect(run?.status).toBe('failed')
  })

  test('does NOT reap a pending/running run that is still within the threshold', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const runId = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })

    // Well within the threshold — e.g. a legitimately long-running job
    await t.run(async ctx => {
      await ctx.db.patch(runId, { startedAt: Date.now() - (STALE_RUN_TIMEOUT_MS - 60_000) })
    })

    const result = await t.mutation(internal.scheduleScanner.scanAndDispatch)
    expect(result.runsReaped).toBe(0)

    const run = await t.query(api.runs.get, { id: runId })
    expect(run?.status).toBe('pending')
    expect(run?.error).toBeUndefined()
  })

  test('a schedule dispatches again once its stuck run is reaped', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)

    const schedId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'wedged-sched',
      cron: '* * * * *',
      enabled: true,
      missedPolicy: 'skip',
    })

    // Schedule is due
    const pastMs = Date.now() - 60_000
    await t.mutation(api.schedules.advanceNextRun, { id: schedId, nextRunAt: pastMs })

    // A stuck run from a previous, long-dead scan is blocking dispatch
    const stuckRunId = await t.mutation(api.runs.create, {
      jobId,
      scheduleId: schedId,
      triggeredBy: 'schedule',
    })
    await t.run(async ctx => {
      await ctx.db.patch(stuckRunId, { startedAt: Date.now() - STALE_RUN_TIMEOUT_MS - 60_000 })
    })

    const result = await t.mutation(internal.scheduleScanner.scanAndDispatch)

    // The stuck run is reaped AND a fresh run is dispatched in the same scan
    expect(result.runsReaped).toBe(1)
    expect(result.runsCreated).toBe(1)

    const stuckRun = await t.query(api.runs.get, { id: stuckRunId })
    expect(stuckRun?.status).toBe('failed')

    const schedule = await t.query(api.schedules.get, { id: schedId })
    expect(schedule?.lastRunStatus).toBe('failed') // reflects the reaped run
    expect(schedule?.nextRunAt).toBeGreaterThan(pastMs)

    const scheduleRuns = await t.query(api.runs.listBySchedule, { scheduleId: schedId })
    expect(scheduleRuns.some(r => r.status === 'pending')).toBe(true)
  })
})

describe('scheduleScanner.scanAndDispatch — stuck-run reaper (job timeout override)', () => {
  test('does NOT reap a running run before its own longer job.config.timeout elapses', async () => {
    const t = convexTest(schema, modules)
    const longTimeoutMs = STALE_RUN_TIMEOUT_MS * 2
    const jobId = await t.mutation(api.jobs.create, {
      ...baseJob,
      config: { command: 'run', timeout: longTimeoutMs },
    })
    const runId = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.markRunning, { id: runId })

    // Past the flat STALE_RUN_TIMEOUT_MS default, but well within this job's
    // own configured timeout + headroom — must not be reaped.
    await t.run(async ctx => {
      await ctx.db.patch(runId, { startedAt: Date.now() - (STALE_RUN_TIMEOUT_MS + 60_000) })
    })

    const result = await t.mutation(internal.scheduleScanner.scanAndDispatch)
    expect(result.runsReaped).toBe(0)

    const run = await t.query(api.runs.get, { id: runId })
    expect(run?.status).toBe('running')
  })

  test('reaps a running run once its own longer job.config.timeout (plus headroom) elapses', async () => {
    const t = convexTest(schema, modules)
    const longTimeoutMs = STALE_RUN_TIMEOUT_MS * 2
    const jobId = await t.mutation(api.jobs.create, {
      ...baseJob,
      config: { command: 'run', timeout: longTimeoutMs },
    })
    const runId = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.markRunning, { id: runId })

    await t.run(async ctx => {
      await ctx.db.patch(runId, {
        startedAt: Date.now() - (longTimeoutMs + RUNNING_TIMEOUT_HEADROOM_MS + 60_000),
      })
    })

    const result = await t.mutation(internal.scheduleScanner.scanAndDispatch)
    expect(result.runsReaped).toBe(1)

    const run = await t.query(api.runs.get, { id: runId })
    expect(run?.status).toBe('failed')
  })

  test('ignores an out-of-range job.config.timeout and falls back to the flat default', async () => {
    const t = convexTest(schema, modules)
    // Above Node's max setTimeout delay (2^31 - 1 ms) — execWorker's exec()
    // can't actually honor this, so the reaper must not treat it as valid.
    const jobId = await t.mutation(api.jobs.create, {
      ...baseJob,
      config: { command: 'run', timeout: MAX_SETTIMEOUT_DELAY_MS + 1 },
    })
    const runId = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.markRunning, { id: runId })

    await t.run(async ctx => {
      await ctx.db.patch(runId, { startedAt: Date.now() - (STALE_RUN_TIMEOUT_MS + 60_000) })
    })

    const result = await t.mutation(internal.scheduleScanner.scanAndDispatch)
    expect(result.runsReaped).toBe(1)

    const run = await t.query(api.runs.get, { id: runId })
    expect(run?.status).toBe('failed')
  })

  test('keeps a large snapshot batch past its legacy one-minute timeout until its account budget', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, {
      ...baseJob,
      config: { command: '__copilot_snapshot__', timeout: 60_000 },
    })
    const accounts = Array.from({ length: 70 }, (_, index) => ({ username: `u${index}`, org: 'o' }))
    const budgetMs = 30_000 + 60_000 * accounts.length
    const runId = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.markRunning, { id: runId })
    await t.run(async ctx => {
      await ctx.db.patch(runId, {
        input: { accounts },
        startedAt: Date.now() - (STALE_RUN_TIMEOUT_MS + 60_000),
      })
    })

    expect((await t.mutation(internal.scheduleScanner.scanAndDispatch)).runsReaped).toBe(0)
    expect((await t.query(api.runs.get, { id: runId }))?.status).toBe('running')

    await t.run(async ctx => {
      await ctx.db.patch(runId, {
        startedAt: Date.now() - (budgetMs + RUNNING_TIMEOUT_HEADROOM_MS + 60_000),
      })
    })
    expect((await t.mutation(internal.scheduleScanner.scanAndDispatch)).runsReaped).toBe(1)
  })
})

describe('scheduleScanner.markSnapshotsDue', () => {
  test('returns zero when no github accounts exist', async () => {
    const t = convexTest(schema, modules)
    const result = await t.mutation(internal.scheduleScanner.markSnapshotsDue)
    expect(result.marked).toBe(0)
  })

  test('creates a pending snapshot run when accounts exist', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.githubAccounts.create, { username: 'user1', org: 'org' })

    const result = await t.mutation(internal.scheduleScanner.markSnapshotsDue)
    expect(result.marked).toBe(1)

    const runs = await t.query(api.runs.listByStatus, { status: 'pending' })
    expect(runs).toHaveLength(1)
    expect(runs[0].triggeredBy).toBe('schedule')
  })

  test('is idempotent — skips if a pending snapshot run exists for today', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.githubAccounts.create, { username: 'user1', org: 'org' })

    await t.mutation(internal.scheduleScanner.markSnapshotsDue)
    const second = await t.mutation(internal.scheduleScanner.markSnapshotsDue)

    expect(second.marked).toBe(0)
    expect(second).toHaveProperty('reason')

    const runs = await t.query(api.runs.listByStatus, { status: 'pending' })
    expect(runs).toHaveLength(1)
  })

  test('auto-creates snapshot job if it does not exist', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.githubAccounts.create, { username: 'user1', org: 'org' })

    await t.mutation(internal.scheduleScanner.markSnapshotsDue)

    const job = await t.query(api.jobs.getByName, { name: 'copilot-usage-snapshot' })
    expect(job).not.toBeNull()
    expect(job?.workerType).toBe('exec')
  })
})
