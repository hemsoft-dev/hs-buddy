import aggregateComponent from '@convex-dev/aggregate/test'
import migrationsComponent from '@convex-dev/migrations/test'
import { convexTest as createConvexTest } from 'convex-test'
import { describe, test, expect, vi } from 'vitest'
import schema from '../schema'
import { api, internal } from '../_generated/api'

const modules = import.meta.glob('../**/*.*s')

function convexTest(schemaDefinition: typeof schema, moduleFiles: typeof modules) {
  const t = createConvexTest(schemaDefinition, moduleFiles)
  aggregateComponent.register(t, 'runCounts')
  migrationsComponent.register(t)
  return t
}

const baseJob = {
  name: 'run-job',
  workerType: 'exec' as const,
  config: { command: 'run' },
}

describe('runs', () => {
  test('anonymous clients can create and list runs', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)

    const id = await t.mutation(api.runs.create, {
      jobId,
      triggeredBy: 'manual',
    })

    expect(id).toBeTruthy()

    const recent = await t.query(api.runs.listRecent, {})
    expect(recent).toHaveLength(1)
    expect(recent[0].status).toBe('pending')
    expect(recent[0].triggeredBy).toBe('manual')
  })

  test('create increments runsTriggered stat', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })

    const stats = await t.query(api.buddyStats.get)
    expect(stats.runsTriggered).toBe(1)
  })

  test('create throws when job does not exist', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    await t.mutation(api.jobs.remove, { id: jobId })

    await expect(t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })).rejects.toThrow()
  })

  test('create rejects a missing schedule before inserting a run or incrementing stats', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const scheduleId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'deleted-schedule',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })
    await t.run(async ctx => {
      await ctx.db.delete(scheduleId)
    })

    await expect(
      t.mutation(api.runs.create, { jobId, scheduleId, triggeredBy: 'schedule' })
    ).rejects.toThrow(/Schedule .* not found/)

    expect(await t.query(api.runs.listRecent, {})).toHaveLength(0)
    expect((await t.query(api.buddyStats.get)).runsTriggered).toBe(0)
  })

  test('create rejects a schedule for another job before inserting a run or incrementing stats', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const otherJobId = await t.mutation(api.jobs.create, {
      ...baseJob,
      name: 'other-job',
    })
    const scheduleId = await t.mutation(api.schedules.create, {
      jobId: otherJobId,
      name: 'other-job-schedule',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })

    await expect(
      t.mutation(api.runs.create, { jobId, scheduleId, triggeredBy: 'schedule' })
    ).rejects.toThrow(/does not belong to job/)

    expect(await t.query(api.runs.listRecent, {})).toHaveLength(0)
    expect((await t.query(api.buddyStats.get)).runsTriggered).toBe(0)
  })

  test('delayed markRunning leaves a completed run unchanged', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })

    await t.mutation(api.runs.complete, { id, output: { result: 'ok' } })
    await t.mutation(api.runs.markRunning, { id })

    const run = await t.query(api.runs.get, { id })
    expect(run?.status).toBe('completed')
    expect(run?.output).toEqual({ result: 'ok' })
    expect(run?.completedAt).toBeGreaterThan(0)
    expect(run?.duration).toBeGreaterThanOrEqual(0)
  })

  test('complete increments runsCompleted stat', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.complete, { id })

    const stats = await t.query(api.buddyStats.get)
    expect(stats.runsCompleted).toBe(1)
  })

  test('delayed markRunning leaves a failed run unchanged', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })

    await t.mutation(api.runs.fail, { id, error: 'something went wrong' })
    await t.mutation(api.runs.markRunning, { id })

    const run = await t.query(api.runs.get, { id })
    expect(run?.status).toBe('failed')
    expect(run?.error).toBe('something went wrong')
    expect(run?.completedAt).toBeGreaterThan(0)
  })

  test('fail increments runsFailed stat', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.fail, { id, error: 'err' })

    const stats = await t.query(api.buddyStats.get)
    expect(stats.runsFailed).toBe(1)
  })

  test('complete updates schedule lastRunStatus when run has a scheduleId', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const schedId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'my-sched',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })
    const runId = await t.mutation(api.runs.create, {
      jobId,
      scheduleId: schedId,
      triggeredBy: 'schedule',
    })
    await t.mutation(api.runs.complete, { id: runId })

    const sched = await t.query(api.schedules.get, { id: schedId })
    expect(sched?.lastRunStatus).toBe('completed')
    expect(sched?.lastRunAt).toBeGreaterThan(0)
  })

  test('fail updates schedule lastRunStatus when run has a scheduleId', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const schedId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'my-sched',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })
    const runId = await t.mutation(api.runs.create, {
      jobId,
      scheduleId: schedId,
      triggeredBy: 'schedule',
    })
    await t.mutation(api.runs.fail, { id: runId, error: 'worker failed' })

    const sched = await t.query(api.schedules.get, { id: schedId })
    expect(sched?.lastRunStatus).toBe('failed')
    expect(sched?.lastRunAt).toBeGreaterThan(0)
  })

  test('complete finalizes a scheduled run after its job and schedule are deleted', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const scheduleId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'deleted-schedule',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })
    const runId = await t.mutation(api.runs.create, {
      jobId,
      scheduleId,
      triggeredBy: 'schedule',
    })
    await t.mutation(api.runs.markRunning, { id: runId })
    await t.mutation(api.jobs.remove, { id: jobId })

    await t.mutation(api.runs.complete, { id: runId, output: { result: 'ok' } })

    const run = await t.query(api.runs.get, { id: runId })
    expect(run).toMatchObject({
      status: 'completed',
      output: { result: 'ok' },
    })
    expect(run?.completedAt).toBeGreaterThan(0)
    expect(run?.duration).toBeGreaterThanOrEqual(0)
    expect((await t.query(api.buddyStats.get)).runsCompleted).toBe(1)
  })

  test('fail finalizes a scheduled run after its job and schedule are deleted', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const scheduleId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'deleted-schedule',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })
    const runId = await t.mutation(api.runs.create, {
      jobId,
      scheduleId,
      triggeredBy: 'schedule',
    })
    await t.mutation(api.runs.markRunning, { id: runId })
    await t.mutation(api.jobs.remove, { id: jobId })

    await t.mutation(api.runs.fail, { id: runId, error: 'worker failed' })

    const run = await t.query(api.runs.get, { id: runId })
    expect(run).toMatchObject({
      status: 'failed',
      error: 'worker failed',
    })
    expect(run?.completedAt).toBeGreaterThan(0)
    expect(run?.duration).toBeGreaterThanOrEqual(0)
    expect((await t.query(api.buddyStats.get)).runsFailed).toBe(1)
  })

  test('delayed markRunning leaves a cancelled run unchanged', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })

    await t.mutation(api.runs.cancel, { id })
    await t.mutation(api.runs.markRunning, { id })

    const run = await t.query(api.runs.get, { id })
    expect(run?.status).toBe('cancelled')
  })

  test('cancel throws when run is already completed', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.complete, { id })

    await expect(t.mutation(api.runs.cancel, { id })).rejects.toThrow(
      'Cannot cancel run with status: completed'
    )
  })

  test('listByJob returns runs for a specific job', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const jobId2 = await t.mutation(api.jobs.create, {
      name: 'other',
      workerType: 'exec',
      config: {},
    })
    await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.create, { jobId: jobId2, triggeredBy: 'manual' })

    const runs = await t.query(api.runs.listByJob, { jobId })
    expect(runs).toHaveLength(1)
  })

  test('listByStatus returns runs with matching status', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id1 = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    const id2 = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.complete, { id: id1 })

    const pending = await t.query(api.runs.listByStatus, { status: 'pending' })
    expect(pending).toHaveLength(1)
    expect(pending[0]._id).toBe(id2)

    const completed = await t.query(api.runs.listByStatus, { status: 'completed' })
    expect(completed).toHaveLength(1)
    expect(completed[0]._id).toBe(id1)
  })

  test('listByStatus can return the oldest pending run first for native claims', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const oldest = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    const newest = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })

    const fifo = await t.query(api.runs.listByStatus, {
      status: 'pending',
      limit: 1,
      oldestFirst: true,
    })
    expect(fifo.map(run => run._id)).toEqual([oldest])
    const latest = await t.query(api.runs.listByStatus, { status: 'pending', limit: 1 })
    expect(latest.map(run => run._id)).toEqual([newest])
  })

  test('claimPending returns null when no pending runs', async () => {
    const t = convexTest(schema, modules)
    const result = await t.mutation(api.runs.claimPending)
    expect(result).toBeNull()
  })

  test('claimPending returns oldest pending run and marks as running', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id1 = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })

    const result = await t.mutation(api.runs.claimPending)
    expect(result).not.toBeNull()
    expect(result?.run._id).toBe(id1)
    expect(result?.run.status).toBe('running')
    expect(result?.job.name).toBe('run-job')

    const run = await t.query(api.runs.get, { id: id1 })
    expect(run?.status).toBe('running')
  })

  test('claimPending fails run when associated job is deleted', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const runId = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })

    // Delete only the job (not via jobs.remove to avoid cascade) to leave the run orphaned
    await t.run(async ctx => {
      await ctx.db.delete(jobId)
    })

    // claimPending should find the pending run, mark it as running, detect the job
    // is missing, then mark the run as failed
    const result = await t.mutation(api.runs.claimPending)
    expect(result).toBeNull()

    // The run should have been failed
    const run = await t.query(api.runs.get, { id: runId })
    expect(run?.status).toBe('failed')
    expect(run?.error).toContain('not found')
  })

  test('cleanup removes old completed runs', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.complete, { id })

    // Use negative days to make cutoff in the future, ensuring all runs are included
    const result = await t.mutation(api.runs.cleanup, { olderThanDays: -1 })
    expect(result.deleted).toBeGreaterThanOrEqual(1)
  })

  test('cleanup does not delete pending or running runs', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })

    // Use negative days to make cutoff in the future
    const result = await t.mutation(api.runs.cleanup, { olderThanDays: -1 })
    expect(result.deleted).toBe(0)

    const recent = await t.query(api.runs.listRecent, {})
    expect(recent).toHaveLength(1)
  })

  test('listBySchedule returns runs for a specific schedule', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const schedId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'my-sched',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })
    const otherSchedId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'other-sched',
      cron: '0 * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })
    await t.mutation(api.runs.create, {
      jobId,
      scheduleId: schedId,
      triggeredBy: 'schedule',
    })
    await t.mutation(api.runs.create, {
      jobId,
      scheduleId: otherSchedId,
      triggeredBy: 'schedule',
    })
    await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })

    const runs = await t.query(api.runs.listBySchedule, { scheduleId: schedId })
    expect(runs).toHaveLength(1)
    expect(runs[0].scheduleId).toBe(schedId)
  })

  test('complete throws when run does not exist', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    // Delete directly to leave orphaned ID
    await t.run(async ctx => {
      await ctx.db.delete(id)
    })

    await expect(t.mutation(api.runs.complete, { id })).rejects.toThrow(/not found/i)
  })

  test('fail throws when run does not exist', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.run(async ctx => {
      await ctx.db.delete(id)
    })

    await expect(t.mutation(api.runs.fail, { id, error: 'err' })).rejects.toThrow(/not found/i)
  })

  test('cancel throws when run does not exist', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.run(async ctx => {
      await ctx.db.delete(id)
    })

    await expect(t.mutation(api.runs.cancel, { id })).rejects.toThrow(/not found/i)
  })

  test('countsByJob aggregates run counts per job', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id1 = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    const id2 = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.complete, { id: id1 })
    await t.mutation(api.runs.fail, { id: id2, error: 'err' })

    const counts = await t.query(api.runs.countsByJob, { jobIds: [jobId] })
    expect(counts[jobId]?.total).toBe(2)
    expect(counts[jobId]?.completed).toBe(1)
    expect(counts[jobId]?.failed).toBe(1)
  })

  test('countsByJob bounds the requested job set', async () => {
    const t = convexTest(schema, modules)
    expect(await t.query(api.runs.countsByJob, { jobIds: [] })).toEqual({})

    const jobIds = []
    for (let index = 0; index <= 100; index++) {
      jobIds.push(
        await t.mutation(api.jobs.create, {
          ...baseJob,
          name: `bounded-count-job-${index}`,
        })
      )
    }

    const atLimit = await t.query(api.runs.countsByJob, { jobIds: jobIds.slice(0, 100) })
    expect(Object.keys(atLimit)).toHaveLength(100)
    expect(atLimit[jobIds[99]]).toEqual({ total: 0, completed: 0, failed: 0 })

    await expect(t.query(api.runs.countsByJob, { jobIds })).rejects.toThrow(/at most 100/)
  })

  test('countsByJob stays exact across terminal transitions and cleanup', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const completedId = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    const failedId = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    const cancelledId = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })

    await t.mutation(api.runs.complete, { id: completedId })
    await t.mutation(api.runs.fail, { id: failedId, error: 'err' })
    await t.mutation(api.runs.cancel, { id: cancelledId })

    expect((await t.query(api.runs.countsByJob, { jobIds: [jobId] }))[jobId]).toEqual({
      total: 4,
      completed: 1,
      failed: 1,
    })

    const cleanup = await t.mutation(api.runs.cleanup, { olderThanDays: -1 })
    expect(cleanup.deleted).toBe(3)
    expect((await t.query(api.runs.countsByJob, { jobIds: [jobId] }))[jobId]).toEqual({
      total: 1,
      completed: 0,
      failed: 0,
    })
  })

  test('bounded backfill returns exact counts beyond 1,000 runs including an older job', async () => {
    const t = convexTest(schema, modules)
    const olderJobId = await t.mutation(api.jobs.create, baseJob)
    const newerJobId = await t.mutation(api.jobs.create, {
      ...baseJob,
      name: 'newer-run-job',
    })

    await t.run(async ctx => {
      await ctx.db.insert('runs', {
        jobId: olderJobId,
        status: 'completed',
        triggeredBy: 'manual',
        startedAt: 1,
      })
      await ctx.db.insert('runs', {
        jobId: olderJobId,
        status: 'failed',
        triggeredBy: 'manual',
        startedAt: 2,
      })
      for (let index = 0; index < 1001; index++) {
        await ctx.db.insert('runs', {
          jobId: newerJobId,
          status: index % 2 === 0 ? 'completed' : 'failed',
          triggeredBy: 'manual',
          startedAt: index + 3,
        })
      }
    })

    await expect(
      t.query(api.runs.countsByJob, { jobIds: [olderJobId, newerJobId] })
    ).rejects.toThrow(/still being backfilled/)

    vi.useFakeTimers()
    try {
      await t.mutation(internal.migrations.runBackfillRunCounts, {})
      await t.finishAllScheduledFunctions(() => vi.runAllTimers())
    } finally {
      vi.useRealTimers()
    }

    const counts = await t.query(api.runs.countsByJob, {
      jobIds: [olderJobId, newerJobId],
    })
    expect(counts[olderJobId]).toEqual({ total: 2, completed: 1, failed: 1 })
    expect(counts[newerJobId]).toEqual({ total: 1001, completed: 501, failed: 500 })
    // The fixture seeds 1,003 records and executes the scheduled backfill under coverage.
    // Keep its count assertions intact while allowing normal CI instrumentation overhead.
  }, 15_000)
})
