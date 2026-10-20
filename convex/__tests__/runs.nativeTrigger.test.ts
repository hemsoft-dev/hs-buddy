import aggregateComponent from '@convex-dev/aggregate/test'
import migrationsComponent from '@convex-dev/migrations/test'
import { convexTest } from 'convex-test'
import { describe, expect, test, vi } from 'vitest'
import { api } from '../_generated/api'
import schema from '../schema'

const modules = import.meta.glob('../**/*.*s')
const requestId = '82f9416a-2708-4e2b-af65-303902211712'
const claimId = '59db3161-ea0f-4589-8ba3-f5d0c70e21b9'
const definition = {
  name: 'native trigger fixture',
  workerType: 'exec' as const,
  config: { command: 'fixture' },
}
function backend() {
  const t = convexTest(schema, modules)
  aggregateComponent.register(t, 'runCounts')
  migrationsComponent.register(t)
  return t
}

describe('native manual-run identity', () => {
  test('concurrent retries create one run and increment the trigger counter once', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, definition)
    const args = { requestId, jobId, input: { task: '雪', nested: { a: 1, b: false } } }
    const [first, second] = await Promise.all([
      t.mutation(api.runs.triggerNative, args),
      t.mutation(api.runs.triggerNative, args),
    ])
    expect(second).toEqual(first)
    expect(first.kind).toBe('recorded')
    expect(await t.query(api.runs.getNativeTrigger, { requestId })).toEqual(first)
    const runs = await t.query(api.runs.listRecent, {})
    expect(runs).toHaveLength(1)
    expect(runs[0].input).toEqual(args.input)
    expect((await t.query(api.buddyStats.get)).runsTriggered).toBe(1)
  })

  test('one request cannot change jobs or inputs while reordered object keys remain equivalent', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, definition)
    const other = await t.mutation(api.jobs.create, { ...definition, name: 'another job' })
    const first = await t.mutation(api.runs.triggerNative, {
      requestId,
      jobId,
      input: { a: 1, b: null },
    })
    expect(
      await t.mutation(api.runs.triggerNative, { requestId, jobId, input: { b: null, a: 1 } })
    ).toEqual(first)
    await expect(
      t.mutation(api.runs.triggerNative, { requestId, jobId: other, input: { a: 1, b: null } })
    ).rejects.toThrow()
    await expect(
      t.mutation(api.runs.triggerNative, { requestId, jobId, input: { a: 2, b: null } })
    ).rejects.toThrow()
    expect(await t.query(api.runs.listRecent, {})).toHaveLength(1)
  })
})

describe('native trigger lossless inputs', () => {
  test('omitted input, null and other falsy values remain distinct', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, definition)
    await t.mutation(api.runs.triggerNative, { requestId, jobId })
    for (const input of [null, false, 0, '']) {
      await expect(t.mutation(api.runs.triggerNative, { requestId, jobId, input })).rejects.toThrow(
        'input does not match'
      )
    }
    expect((await t.query(api.buddyStats.get)).runsTriggered).toBe(1)
  })

  test('Convex bigint and byte values reconcile without changing their types', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, definition)
    const input = { count: 9007199254740993n, bytes: new Uint8Array([0, 255, 17]).buffer }
    const args = { requestId, jobId, input }
    const first = await t.mutation(api.runs.triggerNative, args)
    expect(await t.mutation(api.runs.triggerNative, args)).toEqual(first)
    const [run] = await t.query(api.runs.listRecent, {})
    expect(run.input).toEqual(input)
    await expect(
      t.mutation(api.runs.triggerNative, {
        ...args,
        input: { ...input, count: '9007199254740993' },
      })
    ).rejects.toThrow()
  })
})

describe('native trigger tombstones', () => {
  test('history cleanup never makes an old request enqueue another run', async () => {
    vi.useFakeTimers()
    try {
      const start = Date.parse('2026-10-18T12:00:00Z')
      vi.setSystemTime(start)
      const t = backend()
      const jobId = await t.mutation(api.jobs.create, definition)
      const original = await t.mutation(api.runs.triggerNative, { requestId, jobId, input: null })
      expect(original.kind).toBe('recorded')
      const claimed = await t.mutation(api.runs.claimPending, {})
      expect(claimed).not.toBeNull()
      if (!claimed) throw new Error('Expected the queued fixture run')
      await t.mutation(api.runs.complete, { id: claimed.run._id })
      vi.setSystemTime(start + 31 * 86400000)
      expect(await t.mutation(api.runs.cleanup, { olderThanDays: 30 })).toEqual({ deleted: 1 })
      const repeated = await t.mutation(api.runs.triggerNative, { requestId, jobId, input: null })
      expect(repeated).toEqual({ kind: 'retired', requestId, runId: claimed.run._id })
      expect(await t.query(api.runs.listRecent, {})).toHaveLength(0)
      expect((await t.query(api.buddyStats.get)).runsTriggered).toBe(1)
    } finally {
      vi.useRealTimers()
    }
  })

  test('trigger and execution-claim request identities cannot be interchanged', async () => {
    const t = backend()
    const jobId = await t.mutation(api.jobs.create, definition)
    await t.mutation(api.runs.triggerNative, { requestId, jobId })
    const [run] = await t.query(api.runs.listRecent, {})
    await expect(t.mutation(api.runs.claimNative, { requestId, runId: run._id })).rejects.toThrow()
    await t.mutation(api.runs.claimNative, { requestId: claimId, runId: run._id })
    await expect(
      t.mutation(api.runs.triggerNative, { requestId: claimId, jobId })
    ).rejects.toThrow()
  })
})
