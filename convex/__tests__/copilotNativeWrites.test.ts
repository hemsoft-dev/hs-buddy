import { convexTest } from 'convex-test'
import { makeFunctionReference, type ApiFromModules } from 'convex/server'
import { expect, test } from 'vitest'
import schema from '../schema'
import type * as nativeModule from '../copilotNativeWrites'
import { api } from '../_generated/api'

const modules = import.meta.glob('../**/*.*s')
type NativeApi = ApiFromModules<{ copilotNativeWrites: typeof nativeModule }>['copilotNativeWrites']
const begin: NativeApi['begin'] = makeFunctionReference('copilotNativeWrites:begin')
const claim: NativeApi['claim'] = makeFunctionReference('copilotNativeWrites:claim')
const finish: NativeApi['finish'] = makeFunctionReference('copilotNativeWrites:finish')
const reconcile: NativeApi['reconcile'] = makeFunctionReference('copilotNativeWrites:reconcile')
const requestId = 'aa112233-4455-4677-8899-aabbccddeeff'
const input = { requestId, prompt: 'Review 界', model: 'gpt-4.1', account: 'synthetic-account' }
const review = {
  owner: 'owner',
  repo: 'repo.name',
  prNumber: 7,
  title: 'Synthetic pull request 界',
  head: 'a'.repeat(40),
  threadStats: { total: 4, unresolved: 2, outdated: 1 },
}

test('deleting a running result cannot resurrect its request or authorize another send', async () => {
  const t = convexTest(schema, modules)
  const { id } = await t.mutation(begin, { ...input, review })
  await t.mutation(claim, { id, requestId })
  await t.mutation(api.copilotResults.remove, { id })
  expect(await t.mutation(begin, { ...input, review })).toEqual({
    id,
    created: false,
    status: 'deleted',
  })
  expect(await t.query(reconcile, { requestId })).toEqual({ id, status: 'deleted' })
  await expect(t.mutation(claim, { id, requestId })).rejects.toThrow('identity changed')
  await expect(t.mutation(finish, { id, requestId, result: 'Late answer' })).rejects.toThrow(
    'identity changed'
  )
  expect(await t.query(api.copilotResults.countActive, {})).toEqual({ pending: 0, running: 0 })
  expect(await t.run(async ctx => await ctx.db.query('copilotResults').collect())).toEqual([])
  expect(await t.run(async ctx => await ctx.db.query('prReviewRuns').collect())).toEqual([])
  const retained = await t.run(async ctx => await ctx.db.query('copilotNativeRequests').collect())
  expect(retained).toHaveLength(1)
  expect(retained[0]).not.toHaveProperty('prompt')
})
test('repeated creation retains one result, PR snapshot and active count', async () => {
  const t = convexTest(schema, modules)
  const first = await t.mutation(begin, { ...input, review })
  expect(first).toMatchObject({ created: true, status: 'pending' })
  const retried = await t.mutation(begin, {
    ...input,
    review: { ...review, threadStats: { outdated: 1, unresolved: 2, total: 4 } },
  })
  expect(retried).toEqual({ id: first.id, created: false, status: 'pending' })
  expect(await t.query(api.copilotResults.countActive, {})).toEqual({ pending: 1, running: 0 })
  const rows = await t.run(async ctx => ({
    results: await ctx.db.query('copilotResults').collect(),
    reviews: await ctx.db.query('prReviewRuns').collect(),
  }))
  expect(rows.results).toHaveLength(1)
  expect(rows.reviews).toHaveLength(1)
  expect(rows.reviews[0]).toMatchObject({
    resultId: first.id,
    reviewedHeadSha: review.head,
    reviewedThreadStats: review.threadStats,
    ghAccount: input.account,
  })
})

test('claims acknowledge at most one send and terminal retry changes no counts or timestamps', async () => {
  const t = convexTest(schema, modules)
  const { id } = await t.mutation(begin, { ...input, review })
  expect(await t.mutation(claim, { id, requestId })).toEqual({ claimed: true, status: 'running' })
  expect(await t.mutation(claim, { id, requestId })).toEqual({
    claimed: false,
    status: 'running',
  })
  expect(await t.query(api.copilotResults.countActive, {})).toEqual({ pending: 0, running: 1 })
  const outcome = { id, requestId, result: 'Answer 界' }
  expect(await t.mutation(finish, outcome)).toEqual({ id, status: 'completed', saved: true })
  const saved = await t.query(api.copilotResults.get, { id })
  await t.mutation(finish, outcome)
  expect(await t.query(api.copilotResults.get, { id })).toEqual(saved)
  expect(await t.query(api.copilotResults.countActive, {})).toEqual({ pending: 0, running: 0 })
  const linked = await t.query(api.prReviewRuns.latestByPr, {
    owner: review.owner,
    repo: review.repo,
    prNumber: review.prNumber,
  })
  expect(linked).toMatchObject({ status: 'completed', resultId: id })
  expect(await t.mutation(claim, { id, requestId })).toEqual({
    claimed: false,
    status: 'completed',
  })
  expect(await t.query(reconcile, { requestId })).toEqual({ id, status: 'completed' })
})

test('cancellation retains partial answer in both linked records without replay', async () => {
  const t = convexTest(schema, modules)
  const { id } = await t.mutation(begin, { ...input, review })
  await t.mutation(claim, { id, requestId })
  const outcome = { id, requestId, result: 'Partial 界', failure: 'cancelled' as const }
  await t.mutation(finish, outcome)
  await t.mutation(finish, outcome)
  expect(await t.query(api.copilotResults.get, { id })).toMatchObject({
    status: 'failed',
    error: 'cancelled',
    result: 'Partial 界',
  })
  expect(
    await t.query(api.prReviewRuns.latestByPr, {
      owner: review.owner,
      repo: review.repo,
      prNumber: 7,
    })
  ).toMatchObject({ status: 'failed', error: 'cancelled' })
  await expect(t.mutation(finish, { ...outcome, result: 'Different answer' })).rejects.toThrow(
    'differs from this retry'
  )
  expect(await t.query(api.copilotResults.countActive, {})).toEqual({ pending: 0, running: 0 })
})

test('preflight failure closes pending result; an unstarted success is rejected', async () => {
  const t = convexTest(schema, modules)
  const { id } = await t.mutation(begin, input)
  await expect(t.mutation(finish, { id, requestId, result: 'invented' })).rejects.toThrow(
    'unstarted'
  )
  await t.mutation(finish, { id, requestId, result: '', failure: 'authentication-required' })
  expect(await t.query(api.copilotResults.countActive, {})).toEqual({ pending: 0, running: 0 })
  expect(await t.query(api.copilotResults.get, { id })).toMatchObject({
    status: 'failed',
    error: 'authentication-required',
  })
})

test('creation retries cannot substitute prompt, account, model or PR head', async () => {
  const t = convexTest(schema, modules)
  await t.mutation(begin, { ...input, review })
  const changes = [
    { prompt: 'changed' },
    { account: 'other' },
    { model: 'other' },
    { review: { ...review, head: 'b'.repeat(40) } },
    { review: undefined },
  ]
  for (const change of changes)
    await expect(t.mutation(begin, { ...input, review, ...change })).rejects.toThrow(
      'different content'
    )
  expect(await t.query(api.copilotResults.countActive, {})).toEqual({ pending: 1, running: 0 })
})

test.each([
  { requestId: 'arbitrary' },
  { account: 'bad--account' },
  { account: '-bad' },
  { prompt: '' },
  { prompt: '   ' },
  { prompt: '界'.repeat(90_000) },
  { model: '' },
  { model: 'x'.repeat(257) },
  { model: 'model\n' },
  { review: { ...review, owner: 'bad--owner' } },
  { review: { ...review, repo: '..' } },
  { review: { ...review, repo: 'bad/repo' } },
  { review: { ...review, prNumber: 0 } },
  { review: { ...review, prNumber: 1.5 } },
  { review: { ...review, head: 'invalid' } },
  { review: { ...review, title: 'x'.repeat(4097) } },
  { review: { ...review, threadStats: { total: 1, unresolved: 2, outdated: 0 } } },
  { review: { ...review, threadStats: { total: 1, unresolved: 0, outdated: 2 } } },
  { review: { ...review, threadStats: { total: -1, unresolved: 0, outdated: 0 } } },
])('invalid input creates no result or review', async invalid => {
  const t = convexTest(schema, modules)
  await expect(t.mutation(begin, { ...input, ...invalid })).rejects.toThrow()
  expect(await t.run(async ctx => await ctx.db.query('copilotResults').collect())).toEqual([])
  expect(await t.run(async ctx => await ctx.db.query('prReviewRuns').collect())).toEqual([])
  expect(await t.query(api.copilotResults.countActive, {})).toEqual({ pending: 0, running: 0 })
})

test('wrong identities cannot claim or finish an existing result', async () => {
  const t = convexTest(schema, modules)
  const { id } = await t.mutation(begin, input)
  const wrong = 'bb112233-4455-4677-8899-aabbccddeeff'
  await expect(t.mutation(claim, { id, requestId: wrong })).rejects.toThrow('identity changed')
  await expect(t.mutation(finish, { id, requestId: wrong, result: '' })).rejects.toThrow(
    'identity changed'
  )
  await expect(t.query(reconcile, { requestId: 'invalid' })).rejects.toThrow('request identity')
  expect(await t.query(reconcile, { requestId: wrong })).toBeNull()
  expect(await t.query(api.copilotResults.countActive, {})).toEqual({ pending: 1, running: 0 })
})

test('Unicode output bound rejects oversize writes without changing running state', async () => {
  const t = convexTest(schema, modules)
  const { id } = await t.mutation(begin, input)
  await t.mutation(claim, { id, requestId })
  await expect(t.mutation(finish, { id, requestId, result: '界'.repeat(270_000) })).rejects.toThrow(
    'storage limit'
  )
  expect(await t.query(api.copilotResults.get, { id })).toMatchObject({ status: 'running' })
  expect(await t.query(api.copilotResults.countActive, {})).toEqual({ pending: 0, running: 1 })
})

test('legacy terminal edits cannot be overwritten by a native save retry', async () => {
  const t = convexTest(schema, modules)
  const { id } = await t.mutation(begin, input)
  await t.mutation(api.copilotResults.complete, { id, result: 'Legacy saved answer' })
  await expect(t.mutation(finish, { id, requestId, result: 'Native replacement' })).rejects.toThrow(
    'differs from this retry'
  )
  expect(await t.query(api.copilotResults.get, { id })).toMatchObject({
    result: 'Legacy saved answer',
  })
})
