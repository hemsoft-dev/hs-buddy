import { convexTest } from 'convex-test'
import { makeFunctionReference, type ApiFromModules, type FunctionReturnType } from 'convex/server'
import { expect, test } from 'vitest'
import schema from '../schema'
import type * as nativeModule from '../copilotNative'

const modules = import.meta.glob('../**/*.*s')
type NativeApi = ApiFromModules<{ copilotNative: typeof nativeModule }>['copilotNative']
const list: NativeApi['listResults'] = makeFunctionReference('copilotNative:listResults')
const byStatus: NativeApi['listResultsByStatus'] = makeFunctionReference(
  'copilotNative:listResultsByStatus'
)
const byCategory: NativeApi['listResultsByCategory'] = makeFunctionReference(
  'copilotNative:listResultsByCategory'
)
const reviews: NativeApi['listReviews'] = makeFunctionReference('copilotNative:listReviews')
const usage: NativeApi['listUsage'] = makeFunctionReference('copilotNative:listUsage')
const first = { numItems: 2, cursor: null }

test('usage pages retain the selected account, organization and billing period', async () => {
  const t = convexTest(schema, modules)
  await t.run(async ctx => {
    for (let index = 0; index < 7; index += 1) {
      await ctx.db.insert('copilotUsageHistory', {
        accountUsername: 'synthetic',
        org: 'fixture',
        billingYear: 2026,
        billingMonth: 10,
        premiumRequests: index,
        grossCost: 2,
        discount: 1,
        netCost: 1,
        businessSeats: 1,
        spent: 1,
        snapshotAt: index,
      })
    }
    for (const override of [
      { accountUsername: 'another' },
      { org: 'another' },
      { billingMonth: 9 },
      { billingYear: 2025 },
    ]) {
      await ctx.db.insert('copilotUsageHistory', {
        accountUsername: 'synthetic',
        org: 'fixture',
        billingYear: 2026,
        billingMonth: 10,
        premiumRequests: 99,
        grossCost: 2,
        discount: 1,
        netCost: 1,
        businessSeats: 1,
        spent: 1,
        snapshotAt: 99,
        ...override,
      })
    }
  })
  let cursor: string | null = null
  const seen: number[] = []
  for (let index = 0; index < 4; index += 1) {
    const page: FunctionReturnType<NativeApi['listUsage']> = await t.query(usage, {
      accountUsername: 'synthetic',
      org: 'fixture',
      billingYear: 2026,
      billingMonth: 10,
      paginationOpts: { ...first, cursor },
    })
    seen.push(...page.page.map(row => row.premiumRequests))
    cursor = page.continueCursor
    expect(page.isDone).toBe(index === 3)
  }
  expect(seen).toEqual([6, 5, 4, 3, 2, 1, 0])
})
test('empty history is exhausted without inventing records', async () => {
  const t = convexTest(schema, modules)
  const result = await t.query(list, { paginationOpts: first })
  expect(result.page).toEqual([])
  expect(result.isDone).toBe(true)
})

test('cursor traversal returns every result once in newest-first order', async () => {
  const t = convexTest(schema, modules)
  const ids = await t.run(async ctx => {
    const result = []
    for (let index = 0; index < 7; index += 1) {
      result.push(
        await ctx.db.insert('copilotResults', {
          prompt: `prompt ${index}`,
          status: 'pending',
          createdAt: index,
        })
      )
    }
    return result
  })
  let cursor: string | null = null
  const seen: string[] = []
  for (let pageIndex = 0; pageIndex < 4; pageIndex += 1) {
    const result: FunctionReturnType<NativeApi['listResults']> = await t.query(list, {
      paginationOpts: { ...first, cursor },
    })
    expect(result.page.length).toBeLessThanOrEqual(2)
    seen.push(...result.page.map(row => row._id))
    cursor = result.continueCursor
    expect(result.isDone).toBe(pageIndex === 3)
  }
  expect(seen).toEqual([...ids].reverse())
  expect(new Set(seen).size).toBe(7)
})

test.each([0, -1, 26, 1.5, Number.NaN, Number.POSITIVE_INFINITY])(
  'rejects page size %s',
  async size => {
    const t = convexTest(schema, modules)
    await expect(
      t.query(list, { paginationOpts: { numItems: size, cursor: null } })
    ).rejects.toThrow()
  }
)

test('large private fields stay in the detail record', async () => {
  const t = convexTest(schema, modules)
  const prompt = '😀'.repeat(201) + '\nprivate prompt remainder'
  const id = await t.run(ctx =>
    ctx.db.insert('copilotResults', {
      prompt,
      status: 'completed',
      result: 'private-answer-'.repeat(30_000),
      error: 'private diagnostic',
      metadata: { privateToken: 'must-not-appear-in-summary' },
      model: 'model-1',
      category: 'general',
      createdAt: 1,
      completedAt: 3,
      duration: 2,
    })
  )
  const result = await t.query(list, { paginationOpts: first })
  expect(result.page).toEqual([
    {
      _id: id,
      promptPreview: '😀'.repeat(200) + '…',
      status: 'completed',
      modelLabel: 'model-1',
      categoryLabel: 'general',
      createdAt: 1,
      completedAt: 3,
      duration: 2,
      hasOutput: true,
    },
  ])
  expect(JSON.stringify(result)).not.toContain('private')
  const original = await t.run(ctx => ctx.db.get(id))
  expect(original?.prompt).toBe(prompt)
  expect(original?.result).toBe('private-answer-'.repeat(30_000))
})

test('empty, exact-length and control-character previews preserve Unicode boundaries', async () => {
  const t = convexTest(schema, modules)
  await t.run(async ctx => {
    for (const [index, prompt] of [
      '',
      '界'.repeat(200),
      'a\n\tb\u0000\u007f\u0085\u009f',
    ].entries()) {
      await ctx.db.insert('copilotResults', { prompt, status: 'completed', createdAt: index })
    }
  })
  const result = await t.query(list, { paginationOpts: { numItems: 25, cursor: null } })
  expect(result.page.map(row => row.promptPreview)).toEqual(['a  b    ', '界'.repeat(200), ''])
  expect(result.page.every(row => !row.hasOutput)).toBe(true)
})

test('status and category filters use only their matching records', async () => {
  const t = convexTest(schema, modules)
  await t.run(async ctx => {
    await ctx.db.insert('copilotResults', {
      prompt: 'running',
      status: 'running',
      category: 'general',
      createdAt: 1,
    })
    await ctx.db.insert('copilotResults', {
      prompt: 'failed',
      status: 'failed',
      category: 'pr-review',
      createdAt: 2,
    })
  })
  const running = await t.query(byStatus, { status: 'running', paginationOpts: first })
  const category = await t.query(byCategory, { category: 'pr-review', paginationOpts: first })
  expect(running.page.map(row => row.promptPreview)).toEqual(['running'])
  expect(category.page.map(row => row.promptPreview)).toEqual(['failed'])
})

test('review summaries stay scoped to the PR and omit prompts and diagnostics', async () => {
  const t = convexTest(schema, modules)
  const resultId = await t.run(async ctx => {
    const id = await ctx.db.insert('copilotResults', {
      prompt: 'private prompt',
      status: 'pending',
      createdAt: 1,
    })
    for (const [index, repo] of ['one', 'two'].entries()) {
      await ctx.db.insert('prReviewRuns', {
        owner: 'acme',
        repo,
        prNumber: 7,
        prUrl: `https://github.com/acme/${repo}/pull/7`,
        prTitle: 'Review title',
        status: 'pending',
        resultId: id,
        prompt: 'private review body',
        error: 'private error',
        model: 'model-1',
        ghAccount: 'synthetic-account',
        reviewedHeadSha: 'a'.repeat(40),
        reviewedThreadStats: { total: 3, unresolved: 1, outdated: 0 },
        createdAt: index,
        updatedAt: index,
      })
    }
    return id
  })
  const result = await t.query(reviews, {
    owner: 'acme',
    repo: 'one',
    prNumber: 7,
    paginationOpts: first,
  })
  expect(result.page).toHaveLength(1)
  expect(result.page[0]).toMatchObject({
    resultId,
    status: 'pending',
    titlePreview: 'Review title',
    modelLabel: 'model-1',
    accountLabel: 'synthetic-account',
    reviewedHeadSha: 'a'.repeat(40),
  })
  expect(result.isDone).toBe(true)
  expect(JSON.stringify(result)).not.toContain('private')
})

test('legacy reviews without account, model or captured head remain readable', async () => {
  const t = convexTest(schema, modules)
  const resultId = await t.run(async ctx => {
    const id = await ctx.db.insert('copilotResults', {
      prompt: '',
      status: 'failed',
      createdAt: 1,
    })
    await ctx.db.insert('prReviewRuns', {
      owner: 'acme',
      repo: 'legacy',
      prNumber: 8,
      prUrl: 'https://github.com/acme/legacy/pull/8',
      prTitle: '',
      status: 'failed',
      resultId: id,
      prompt: 'private older prompt',
      error: 'private older error',
      createdAt: 1,
      updatedAt: 2,
      completedAt: 2,
    })
    return id
  })
  const result = await t.query(reviews, {
    owner: 'acme',
    repo: 'legacy',
    prNumber: 8,
    paginationOpts: first,
  })
  expect(result.page[0]).toMatchObject({
    resultId,
    titlePreview: '',
    status: 'failed',
    completedAt: 2,
  })
  expect(result.page[0]).not.toHaveProperty('modelLabel')
  expect(result.page[0]).not.toHaveProperty('accountLabel')
  expect(result.page[0]).not.toHaveProperty('reviewedHeadSha')
  expect(JSON.stringify(result)).not.toContain('private')
})

test.each([
  { stored: 'private-invalid-head'.repeat(10_000), expected: undefined },
  { stored: 'f'.repeat(64), expected: 'f'.repeat(64) },
])('legacy captured head is bounded and supports SHA-256', async ({ stored, expected }) => {
  const t = convexTest(schema, modules)
  await t.run(async ctx => {
    const resultId = await ctx.db.insert('copilotResults', {
      prompt: '',
      status: 'pending',
      createdAt: 1,
    })
    await ctx.db.insert('prReviewRuns', {
      owner: 'acme',
      repo: 'heads',
      prNumber: 9,
      prUrl: 'https://github.com/acme/heads/pull/9',
      prTitle: 'Captured head',
      status: 'pending',
      resultId,
      prompt: '',
      reviewedHeadSha: stored,
      createdAt: 1,
      updatedAt: 1,
    })
  })
  const result = await t.query(reviews, {
    owner: 'acme',
    repo: 'heads',
    prNumber: 9,
    paginationOpts: first,
  })
  expect(result.page[0].reviewedHeadSha).toBe(expected)
  expect(JSON.stringify(result).length).toBeLessThan(2000)
  expect(JSON.stringify(result)).not.toContain('private')
})
