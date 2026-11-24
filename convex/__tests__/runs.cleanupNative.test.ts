import aggregateComponent from '@convex-dev/aggregate/test'
import migrationsComponent from '@convex-dev/migrations/test'
import { convexTest } from 'convex-test'
import { describe, expect, test } from 'vitest'
import { api } from '../_generated/api'
import schema from '../schema'

const modules = import.meta.glob('../**/*.*s')
const requestId = '3b0c7a52-8f1e-4d6a-9c2b-5e7f1a4d8c90'
const otherRequestId = '7d4e2f10-6a3b-4c8d-a1e5-0f9b2c7d3e64'
const definition = {
  name: 'cleanup fixture',
  workerType: 'exec' as const,
  config: { command: 'x' },
}

function backend() {
  const t = convexTest(schema, modules)
  aggregateComponent.register(t, 'runCounts')
  migrationsComponent.register(t)
  return t
}

async function finishedRuns(t: ReturnType<typeof backend>, count: number) {
  const jobId = await t.mutation(api.jobs.create, definition)
  for (let index = 0; index < count; index++) {
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.complete, { id })
  }
  return jobId
}

describe('native run cleanup', () => {
  // A negative age puts the cutoff in the future, so every run is old enough.
  test('a repeated request returns its first result and deletes nothing more', async () => {
    const t = backend()
    const jobId = await finishedRuns(t, 2)
    const first = await t.mutation(api.runs.cleanupNative, { requestId, olderThanDays: -1 })
    expect(first).toEqual({ deleted: 2, more: false })

    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.complete, { id })
    const replay = await t.mutation(api.runs.cleanupNative, { requestId, olderThanDays: -1 })
    expect(replay).toEqual({ deleted: 2, more: false })
    expect(await t.query(api.runs.listRecent, {})).toHaveLength(1)

    const next = await t.mutation(api.runs.cleanupNative, {
      requestId: otherRequestId,
      olderThanDays: -1,
    })
    expect(next).toEqual({ deleted: 1, more: false })
  })

  test('keeps pending and recent runs and rejects a reused or malformed identity', async () => {
    const t = backend()
    const jobId = await finishedRuns(t, 1)
    await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    const recent = await t.mutation(api.runs.cleanupNative, { requestId, olderThanDays: 7 })
    expect(recent).toEqual({ deleted: 0, more: false })
    expect(await t.query(api.runs.listRecent, {})).toHaveLength(2)
    // An empty cleanup changed nothing and keeps no receipt.
    expect(await t.run(ctx => ctx.db.query('automationCleanupReceipts').collect())).toEqual([])

    await expect(
      t.mutation(api.runs.cleanupNative, { requestId: 'not-a-uuid', olderThanDays: -1 })
    ).rejects.toThrow()
    const old = await t.mutation(api.runs.cleanupNative, { requestId, olderThanDays: -1 })
    expect(old).toEqual({ deleted: 1, more: false })
    await expect(
      t.mutation(api.runs.cleanupNative, { requestId, olderThanDays: 7 })
    ).rejects.toThrow('different age')
    const left = await t.query(api.runs.listRecent, {})
    expect(left.map(run => run.status)).toEqual(['pending'])
  })
})

describe('native run cleanup bounds', () => {
  test('prunes receipts older than a day', async () => {
    const t = backend()
    await t.run(async ctx => {
      for (const [id, age] of [
        [otherRequestId, 2 * 86_400_000],
        ['5c6d7e8f-9a0b-4c1d-8e2f-3a4b5c6d7e8f', 60_000],
      ] as const) {
        await ctx.db.insert('automationCleanupReceipts', {
          requestId: id,
          olderThanDays: 7,
          deleted: 1,
          more: false,
          createdAt: Date.now() - age,
        })
      }
    })
    await t.mutation(api.runs.cleanupNative, { requestId, olderThanDays: 7 })
    const kept = await t.run(ctx => ctx.db.query('automationCleanupReceipts').collect())
    expect(kept.map(receipt => receipt.requestId)).toEqual(['5c6d7e8f-9a0b-4c1d-8e2f-3a4b5c6d7e8f'])
  })

  test('counts the read budget in UTF-8 bytes', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, definition)
    // 300 Ki three-byte characters: 900 KiB stored, 300 Ki UTF-16 units.
    const output = '界'.repeat(300 * 1024)
    for (let index = 0; index < 12; index++) {
      const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
      await t.mutation(api.runs.complete, { id, output })
    }
    const first = await t.mutation(api.runs.cleanupNative, { requestId, olderThanDays: -1 })
    expect(first.more).toBe(true)
    expect(first.deleted).toBeLessThan(12)
  }, 60_000)

  test('stops at the read budget when runs carry large output', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, definition)
    const output = 'x'.repeat(900 * 1024)
    for (let index = 0; index < 12; index++) {
      const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
      await t.mutation(api.runs.complete, { id, output })
    }
    const first = await t.mutation(api.runs.cleanupNative, { requestId, olderThanDays: -1 })
    expect(first.more).toBe(true)
    expect(first.deleted).toBeGreaterThan(0)
    expect(first.deleted).toBeLessThan(12)
    const second = await t.mutation(api.runs.cleanupNative, {
      requestId: otherRequestId,
      olderThanDays: -1,
    })
    expect(first.deleted + second.deleted).toBe(12)
    expect(second.more).toBe(false)
  }, 60_000)

  test('old pending runs never hold back a finished run behind them', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, definition)
    for (let index = 0; index < 501; index++) {
      await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    }
    const id = await t.mutation(api.runs.create, { jobId, triggeredBy: 'manual' })
    await t.mutation(api.runs.complete, { id })
    const result = await t.mutation(api.runs.cleanupNative, { requestId, olderThanDays: -1 })
    expect(result).toEqual({ deleted: 1, more: false })
    expect(await t.query(api.runs.get, { id })).toBeNull()
  }, 60_000)

  test('deletes one bounded batch and reports that older runs remain', async () => {
    const t = backend()
    await finishedRuns(t, 501)
    const first = await t.mutation(api.runs.cleanupNative, { requestId, olderThanDays: -1 })
    expect(first).toEqual({ deleted: 500, more: true })
    const second = await t.mutation(api.runs.cleanupNative, {
      requestId: otherRequestId,
      olderThanDays: -1,
    })
    expect(second).toEqual({ deleted: 1, more: false })
  }, 60_000)
})
