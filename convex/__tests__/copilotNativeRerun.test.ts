import { convexTest } from 'convex-test'
import { makeFunctionReference, type ApiFromModules } from 'convex/server'
import { expect, test } from 'vitest'
import schema from '../schema'
import type * as nativeModule from '../copilotNativeWrites'
import { api } from '../_generated/api'

const modules = import.meta.glob('../**/*.*s')
type NativeApi = ApiFromModules<{ copilotNativeWrites: typeof nativeModule }>['copilotNativeWrites']
const begin: NativeApi['begin'] = makeFunctionReference('copilotNativeWrites:begin')
const firstRequest = 'aa112233-4455-4677-8899-aabbccddeeff'
const rerunRequest = 'bb112233-4455-4677-8899-aabbccddeeff'
const input = { requestId: firstRequest, prompt: 'Explain 界', model: 'gpt-4.1', account: 'synth' }
const review = {
  owner: 'owner',
  repo: 'repo',
  prNumber: 7,
  title: 'Synthetic pull request',
  head: 'a'.repeat(40),
  threadStats: { total: 3, unresolved: 1, outdated: 0 },
}

async function rows(t: ReturnType<typeof convexTest>) {
  return t.run(async ctx => ({
    results: await ctx.db.query('copilotResults').collect(),
    reviews: await ctx.db.query('prReviewRuns').collect(),
  }))
}

test('the chosen prompt category is stored and a retry of the same request keeps one result', async () => {
  const t = convexTest(schema, modules)
  const first = await t.mutation(begin, { ...input, category: 'documentation' })
  expect(first).toMatchObject({ created: true, status: 'pending' })
  expect(await t.mutation(begin, { ...input, category: 'documentation' })).toEqual({
    id: first.id,
    created: false,
    status: 'pending',
  })
  await expect(t.mutation(begin, { ...input, category: 'code-analysis' })).rejects.toThrow(
    'different content'
  )
  await expect(t.mutation(begin, input)).rejects.toThrow('different content')
  const { results } = await rows(t)
  expect(results).toHaveLength(1)
  expect(results[0]).toMatchObject({ category: 'documentation', metadata: { ghAccount: 'synth' } })
  expect(await t.query(api.copilotResults.countActive, {})).toEqual({ pending: 1, running: 0 })
})

test('a request without a category stays general and a captured review stays a PR Review', async () => {
  const t = convexTest(schema, modules)
  await t.mutation(begin, input)
  await expect(
    t.mutation(begin, {
      ...input,
      requestId: rerunRequest,
      review,
      category: 'documentation',
    })
  ).rejects.toThrow('always a PR Review')
  await t.mutation(begin, { ...input, requestId: rerunRequest, review, category: 'pr-review' })
  const { results } = await rows(t)
  expect(results.map(row => row.category).sort()).toEqual(['general', 'pr-review'])
})

test('a re-run copies the saved category and metadata but adds no AI Reviews entry', async () => {
  const t = convexTest(schema, modules)
  const source = await t.mutation(begin, { ...input, review })
  const rerun = { ...input, requestId: rerunRequest, rerunOf: source.id }
  const created = await t.mutation(begin, rerun)
  expect(created).toMatchObject({ created: true, status: 'pending' })
  expect(created.id).not.toBe(source.id)
  expect(await t.mutation(begin, rerun)).toEqual({
    id: created.id,
    created: false,
    status: 'pending',
  })
  const { results, reviews } = await rows(t)
  const copy = results.find(row => row._id === created.id)
  const original = results.find(row => row._id === source.id)
  expect(copy).toMatchObject({ prompt: input.prompt, category: 'pr-review' })
  expect(copy?.metadata).toEqual(original?.metadata)
  // Without tools a native re-run never sees the diff, so it must not stand
  // for a review of the saved head in the PR's AI Reviews history.
  expect(reviews).toHaveLength(1)
  expect(reviews.find(row => row.resultId === created.id)).toBeUndefined()
  expect(await t.query(api.copilotResults.countActive, {})).toEqual({ pending: 2, running: 0 })
})

test('a re-run of an Electron result keeps its metadata and category as saved', async () => {
  const t = convexTest(schema, modules)
  const id = await t.mutation(api.copilotResults.create, {
    prompt: input.prompt,
    category: 'code-analysis',
    metadata: { ghAccount: 'Synth', note: 'kept' },
  })
  const created = await t.mutation(begin, { ...input, requestId: rerunRequest, rerunOf: id })
  const { results, reviews } = await rows(t)
  expect(results.find(row => row._id === created.id)).toMatchObject({
    category: 'code-analysis',
    metadata: { ghAccount: 'synth', note: 'kept' },
  })
  expect(reviews).toEqual([])
  const bare = await t.mutation(api.copilotResults.create, { prompt: input.prompt })
  const second = await t.mutation(begin, {
    ...input,
    requestId: 'cc112233-4455-4677-8899-aabbccddeeff',
    rerunOf: bare,
  })
  const after = await rows(t)
  const stored = after.results.find(row => row._id === second.id)
  expect(stored?.category).toBeUndefined()
  expect(stored?.metadata).toEqual({ ghAccount: 'synth' })
})

test('a blank or padded saved account reads as the native client reads it', async () => {
  const t = convexTest(schema, modules)
  const ids = ['dd', 'ee', 'ff'].map(prefix => `${prefix}112233-4455-4677-8899-aabbccddeeff`)
  for (const [index, ghAccount] of ['', '   ', '  Synth  '].entries()) {
    const id = await t.mutation(api.copilotResults.create, {
      prompt: input.prompt,
      metadata: { ghAccount },
    })
    const created = await t.mutation(begin, { ...input, requestId: ids[index], rerunOf: id })
    const { results } = await rows(t)
    expect(results.find(row => row._id === created.id)?.metadata).toEqual({ ghAccount: 'synth' })
  }
})

test('a re-run refuses a changed prompt, another account, extra fields or a missing source', async () => {
  const t = convexTest(schema, modules)
  const source = await t.mutation(begin, input)
  const rerun = { ...input, requestId: rerunRequest, rerunOf: source.id }
  await expect(t.mutation(begin, { ...rerun, prompt: 'Something else' })).rejects.toThrow(
    'saved prompt'
  )
  await expect(t.mutation(begin, { ...rerun, account: 'other' })).rejects.toThrow('saved account')
  await expect(t.mutation(begin, { ...rerun, category: 'general' })).rejects.toThrow(
    'keeps the saved result'
  )
  await expect(t.mutation(begin, { ...rerun, review })).rejects.toThrow('keeps the saved result')
  await t.mutation(api.copilotResults.remove, { id: source.id })
  await expect(t.mutation(begin, rerun)).rejects.toThrow('no longer exists')
  expect((await rows(t)).results).toEqual([])
})
