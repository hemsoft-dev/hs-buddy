import aggregateComponent from '@convex-dev/aggregate/test'
import migrationsComponent from '@convex-dev/migrations/test'
import { convexTest } from 'convex-test'
import { describe, expect, test, vi } from 'vitest'
import { api } from '../_generated/api'
import schema from '../schema'

const modules = import.meta.glob('../**/*.*s')
function backend() {
  const t = convexTest(schema, modules)
  aggregateComponent.register(t, 'runCounts')
  migrationsComponent.register(t)
  return t
}
const requestId = '2cd2e788-58ae-493e-90eb-449b6c726a77'
const anotherId = 'c1b97af3-8983-4b74-bbdd-a545cb87ecc7'
const job = {
  name: 'native owned fixture',
  workerType: 'exec' as const,
  config: { command: 'fixture' },
}

describe('native automation claim retention', () => {
  test('a failed native command retains its captured stdout and stderr exactly once', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, job)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.claimNative, { requestId, runId: id })
    const output = { stdout: '雪\npartial stdout', stderr: 'exact stderr\n', exitCode: 7 }
    await t.mutation(api.runs.fail, {
      id,
      claimRequestId: requestId,
      error: 'Process exited with code 7',
      output,
    })
    await t.mutation(api.runs.fail, {
      id,
      claimRequestId: requestId,
      error: 'late result',
      output: 'overwrite',
    })
    const failed = await t.query(api.runs.get, { id })
    expect(failed?.status).toBe('failed')
    expect(failed?.output).toEqual(output)
    expect(failed?.error).toBe('Process exited with code 7')
    expect((await t.query(api.buddyStats.get)).runsFailed).toBe(1)
  })

  test('a delayed claim cannot consume a replacement when its captured candidate was taken', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, job)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    expect((await t.mutation(api.runs.claimPending, {}))?.run._id).toBe(id)
    const next = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    expect(await t.mutation(api.runs.claimNative, { requestId, runId: id })).toEqual({
      kind: 'retired',
      requestId,
      runId: id,
    })
    expect(await t.mutation(api.runs.claimNative, { requestId, runId: id })).toEqual({
      kind: 'retired',
      requestId,
      runId: id,
    })
    await expect(t.mutation(api.runs.claimNative, { requestId, runId: next })).rejects.toThrow(
      'candidate'
    )
    expect((await t.query(api.runs.get, { id: next }))?.status).toBe('pending')
  })

  test('history cleanup drops captured job data while keeping the claim tombstone', async () => {
    vi.useFakeTimers()
    try {
      const start = Date.parse('2026-10-18T12:00:00Z')
      vi.setSystemTime(start)
      const t = backend()
      const jobId = await t.mutation(api.jobs.create, job)
      const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
      await t.mutation(api.runs.claimNative, { requestId, runId: id })
      await t.mutation(api.runs.complete, { id, claimRequestId: requestId })
      vi.setSystemTime(start + 31 * 86400000)
      expect(await t.mutation(api.runs.cleanup, { olderThanDays: 30 })).toEqual({ deleted: 1 })
      const tombstone = await t.run(ctx =>
        ctx.db
          .query('automationReceipts')
          .withIndex('by_request', q => q.eq('requestId', requestId))
          .unique()
      )
      expect(tombstone?.runId).toBe(id)
      expect(tombstone?.job).toBeUndefined()
      expect(await t.mutation(api.runs.claimNative, { requestId, runId: id })).toEqual({
        kind: 'retired',
        requestId,
        runId: id,
      })
    } finally {
      vi.useRealTimers()
    }
  })
})

describe('native automation claim selection', () => {
  test('a repeated claim keeps its original run and captured job after edits', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, job)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    const first = await t.mutation(api.runs.claimNative, { requestId, runId: id })
    expect(first.kind).toBe('claimed')
    if (first.kind !== 'claimed') throw new Error('Expected the original claim')
    expect(first.run._id).toBe(id)
    await t.mutation(api.jobs.update, { id: jobId, config: { command: 'changed later' } })
    const nextId = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    const repeated = await t.mutation(api.runs.claimNative, { requestId, runId: id })
    if (repeated.kind !== 'claimed') throw new Error('Expected the same captured claim')
    expect(repeated.run._id).toBe(id)
    expect(repeated.job.config.command).toBe('fixture')
    expect((await t.query(api.runs.get, { id: nextId }))?.status).toBe('pending')
  })

  test('Electron and native requests compete for one atomic pending claim', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, job)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    const [electron, native] = await Promise.all([
      t.mutation(api.runs.claimPending, {}),
      t.mutation(api.runs.claimNative, { requestId, runId: id }),
    ])
    const claimed = [
      electron?.run._id,
      native.kind === 'claimed' ? native.run._id : undefined,
    ].filter(Boolean)
    expect(claimed).toEqual([id])
    expect((await t.query(api.runs.get, { id }))?.status).toBe('running')
  })
})

describe('native automation claim ownership and finalization', () => {
  test('wrong-owner and legacy completions cannot finalize a native claim', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, job)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.claimNative, { requestId, runId: id })
    for (const claimRequestId of [undefined, anotherId]) {
      await expect(t.mutation(api.runs.complete, { id, claimRequestId })).rejects.toThrow()
      await expect(
        t.mutation(api.runs.fail, { id, claimRequestId, error: 'wrong owner' })
      ).rejects.toThrow()
    }
    expect((await t.query(api.runs.get, { id }))?.status).toBe('running')
    expect((await t.query(api.buddyStats.get)).runsCompleted).toBe(0)
    await t.mutation(api.runs.complete, {
      id,
      claimRequestId: requestId,
      output: { stdout: 'one execution' },
    })
    await t.mutation(api.runs.complete, {
      id,
      claimRequestId: requestId,
      output: { stdout: 'late overwrite' },
    })
    const done = await t.query(api.runs.get, { id })
    expect(done?.output).toEqual({ stdout: 'one execution' })
    expect((await t.query(api.buddyStats.get)).runsCompleted).toBe(1)
    expect(done).not.toHaveProperty('nativeClaimRequestId')
  })

  test('a deleted claimed run leaves a tombstone and never claims its replacement', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, job)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.claimNative, { requestId, runId: id })
    await t.run(async ctx => {
      await ctx.db.delete(id)
    })
    const next = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    const lookup = await t.query(api.runs.getNativeClaim, { requestId })
    expect(lookup).toEqual({ kind: 'retired', requestId, runId: id })
    expect(await t.mutation(api.runs.claimNative, { requestId, runId: id })).toEqual(lookup)
    expect((await t.query(api.runs.get, { id: next }))?.status).toBe('pending')
  })

  test('cancellation stays terminal when the original worker finishes later', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, job)
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.claimNative, { requestId, runId: id })
    await t.mutation(api.runs.cancel, { id })
    await t.mutation(api.runs.complete, { id, claimRequestId: requestId, output: 'late' })
    await t.mutation(api.runs.complete, { id, claimRequestId: requestId, output: 'overwrite' })
    const cancelled = await t.query(api.runs.get, { id })
    expect(cancelled?.status).toBe('cancelled')
    expect(cancelled?.output).toBe('late')
    expect((await t.query(api.buddyStats.get)).runsCompleted).toBe(0)
  })
})
