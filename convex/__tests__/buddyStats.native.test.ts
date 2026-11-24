import { convexTest } from 'convex-test'
import { test, expect } from 'vitest'
import schema from '../schema'
import { api } from '../_generated/api'

const modules = import.meta.glob('../**/*.*s')

const requestId = '1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d'
const otherRequestId = '5c6d7e8f-9a0b-4c1d-8e2f-3a4b5c6d7e8f'
const owner = '9f8e7d6c-5b4a-4c3d-9e2f-1a0b9c8d7e6f'
const otherOwner = '0a1b2c3d-4e5f-4a6b-bc7d-8e9f0a1b2c3d'

const backend = () => convexTest(schema, modules)
type Backend = ReturnType<typeof backend>

async function stored(t: Backend) {
  return t.run(ctx =>
    ctx.db
      .query('buddyStats')
      .withIndex('by_key', q => q.eq('key', 'default'))
      .first()
  )
}

/** Move the open session's start a minute back, so a flush adds uptime. */
async function backdateSession(t: Backend) {
  const doc = await stored(t)
  await t.run(ctx =>
    ctx.db.patch('buddyStats', doc!._id, { lastSessionStart: Date.now() - 60_000 })
  )
}

test('incrementNative counts a repeated requestId once', async () => {
  const t = backend()
  const fields = { tabsOpened: 1, prsViewed: 1 }
  expect(await t.mutation(api.buddyStats.incrementNative, { requestId, fields })).toEqual({
    counted: true,
  })
  // A resend after a reconnect carries the same identity.
  expect(await t.mutation(api.buddyStats.incrementNative, { requestId, fields })).toEqual({
    counted: false,
  })
  await t.mutation(api.buddyStats.incrementNative, {
    requestId: otherRequestId,
    fields: { tabsOpened: 1 },
  })

  const stats = await t.query(api.buddyStats.get)
  expect(stats.tabsOpened).toBe(2)
  expect(stats.prsViewed).toBe(1)
})

test('incrementNative validates the request identity and fields before counting', async () => {
  const t = backend()
  await expect(
    t.mutation(api.buddyStats.incrementNative, {
      requestId: 'not-a-uuid',
      fields: { tabsOpened: 1 },
    })
  ).rejects.toThrow('UUID request identity')
  await expect(
    t.mutation(api.buddyStats.incrementNative, { requestId, fields: { badField: 1 } })
  ).rejects.toThrow('Invalid stat field: badField')

  // Neither failure kept a receipt, so the identity still counts.
  await t.mutation(api.buddyStats.incrementNative, { requestId, fields: { tabsOpened: 1 } })
  expect((await t.query(api.buddyStats.get)).tabsOpened).toBe(1)
})

test('incrementNative prunes receipts older than a day', async () => {
  const t = backend()
  await t.run(async ctx => {
    for (const [id, age] of [
      [otherRequestId, 2 * 86_400_000],
      ['7b8c9d0e-1f2a-4b3c-9d4e-5f6a7b8c9d0e', 60_000],
    ] as const) {
      await ctx.db.insert('buddyStatsReceipts', { requestId: id, createdAt: Date.now() - age })
    }
  })
  await t.mutation(api.buddyStats.incrementNative, { requestId, fields: { tabsOpened: 1 } })
  const kept = await t.run(ctx => ctx.db.query('buddyStatsReceipts').collect())
  expect(kept.map(receipt => receipt.requestId).sort()).toEqual(
    [requestId, '7b8c9d0e-1f2a-4b3c-9d4e-5f6a7b8c9d0e'].sort()
  )
})

test('a native start counts one launch and a resend keeps it', async () => {
  const t = backend()
  expect(await t.mutation(api.buddyStats.recordSessionStartNative, { owner })).toEqual({
    started: true,
  })
  const first = await stored(t)
  expect(first?.sessionOwner).toBe(owner)
  expect(first?.appLaunches).toBe(1)

  expect(await t.mutation(api.buddyStats.recordSessionStartNative, { owner })).toEqual({
    started: true,
  })
  const resent = await stored(t)
  expect(resent?.appLaunches).toBe(1)
  expect(resent?.lastSessionStart).toBe(first?.lastSessionStart)
})

test("a native start leaves another client's open session alone", async () => {
  const t = backend()
  await t.mutation(api.buddyStats.recordSessionStart)
  const before = await stored(t)
  expect(await t.mutation(api.buddyStats.recordSessionStartNative, { owner })).toEqual({
    started: false,
  })
  await t.mutation(api.buddyStats.recordSessionStartNative, { owner: otherOwner })
  expect(await stored(t)).toEqual(before)
})

test("native checkpoint and end never touch another client's session", async () => {
  const t = backend()
  await t.mutation(api.buddyStats.recordSessionStartNative, { owner: otherOwner })
  await backdateSession(t)
  const before = await stored(t)
  await t.mutation(api.buddyStats.checkpointUptimeNative, { owner })
  await t.mutation(api.buddyStats.recordSessionEndNative, { owner })
  expect(await stored(t)).toEqual(before)

  // Electron's session (no owner) is not the native app's either.
  const electron = backend()
  await electron.mutation(api.buddyStats.recordSessionStart)
  const electronBefore = await stored(electron)
  await electron.mutation(api.buddyStats.recordSessionEndNative, { owner })
  expect(await stored(electron)).toEqual(electronBefore)
})

test('the owner checkpoints its uptime and its end clears the owner', async () => {
  const t = backend()
  await t.mutation(api.buddyStats.recordSessionStartNative, { owner })
  await backdateSession(t)
  await t.mutation(api.buddyStats.checkpointUptimeNative, { owner })
  const checkpointed = await stored(t)
  expect(checkpointed?.totalUptimeMs).toBeGreaterThanOrEqual(60_000)
  expect(checkpointed?.sessionOwner).toBe(owner)

  await t.mutation(api.buddyStats.recordSessionEndNative, { owner })
  const ended = await stored(t)
  expect(ended?.lastSessionStart).toBeUndefined()
  expect(ended?.sessionOwner).toBeUndefined()

  // The next launch opens a new session and counts it.
  expect(await t.mutation(api.buddyStats.recordSessionStartNative, { owner: otherOwner })).toEqual({
    started: true,
  })
  expect((await stored(t))?.appLaunches).toBe(2)
})

test('an Electron session after a native one does not inherit the native owner', async () => {
  const t = backend()
  await t.mutation(api.buddyStats.recordSessionStartNative, { owner })
  // Electron's quit ends any open session, including a native one.
  await t.mutation(api.buddyStats.recordSessionEnd)
  expect((await stored(t))?.sessionOwner).toBeUndefined()
  await t.mutation(api.buddyStats.recordSessionStart)
  expect(await t.mutation(api.buddyStats.recordSessionStartNative, { owner })).toEqual({
    started: false,
  })
})

test('native session functions require a UUID owner', async () => {
  const t = backend()
  for (const fn of [
    api.buddyStats.recordSessionStartNative,
    api.buddyStats.checkpointUptimeNative,
    api.buddyStats.recordSessionEndNative,
  ]) {
    await expect(t.mutation(fn, { owner: 'launch' })).rejects.toThrow('UUID request identity')
  }
})
