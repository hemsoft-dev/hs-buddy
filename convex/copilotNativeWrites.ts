import { v } from 'convex/values'
import type { Doc, Id } from './_generated/dataModel'
import { mutation, query, type MutationCtx } from './_generated/server'
import { transitionResultCount } from './lib/copilotResultCounts'

const reviewValidator = v.object({
  owner: v.string(),
  repo: v.string(),
  prNumber: v.number(),
  title: v.string(),
  head: v.string(),
  threadStats: v.object({ total: v.number(), unresolved: v.number(), outdated: v.number() }),
})
const failureValidator = v.union(
  v.literal('cancelled'),
  v.literal('authentication-required'),
  v.literal('service-unavailable'),
  v.literal('delivery-unknown'),
  v.literal('incomplete-stream'),
  v.literal('output-limit')
)
// Electron's prompt box offers these four categories.
const categoryValidator = v.union(
  v.literal('general'),
  v.literal('pr-review'),
  v.literal('code-analysis'),
  v.literal('documentation')
)

function requestId(value: string) {
  if (!/^[a-f\d]{8}-[a-f\d]{4}-4[a-f\d]{3}-[89ab][a-f\d]{3}-[a-f\d]{12}$/i.test(value)) {
    throw new Error('Invalid assistant request identity.')
  }
}

function textLimit(value: string, maximum: number) {
  if (new TextEncoder().encode(value).byteLength > maximum) {
    throw new Error('Assistant content exceeds its storage limit.')
  }
}

function account(value: string) {
  if (!/^[a-z\d](?:[a-z\d-]{0,37}[a-z\d])?$/i.test(value) || value.includes('--')) {
    throw new Error('Choose a valid GitHub account.')
  }
}

type Review = {
  owner: string
  repo: string
  prNumber: number
  title: string
  head: string
  threadStats: { total: number; unresolved: number; outdated: number }
}

function reviewContext(review: Review) {
  account(review.owner)
  if (!/^[\w.-]{1,100}$/.test(review.repo) || review.repo === '.' || review.repo === '..') {
    throw new Error('Choose a valid repository.')
  }
  if (!Number.isSafeInteger(review.prNumber) || review.prNumber < 1) {
    throw new Error('Choose a valid pull request.')
  }
  if (!/^(?:[a-f\d]{40}|[a-f\d]{64})$/i.test(review.head)) {
    throw new Error('Capture the pull request head before reviewing.')
  }
  textLimit(review.title, 4096)
  const { total, unresolved, outdated } = review.threadStats
  if (
    ![total, unresolved, outdated].every(value => Number.isSafeInteger(value) && value >= 0) ||
    unresolved > total ||
    outdated > total
  ) {
    throw new Error('Invalid captured review thread counts.')
  }
}

// Electron stores the PR link in result metadata; its results list and header open it.
function pullRequestUrl(review: Review) {
  return `https://github.com/${review.owner}/${review.repo}/pull/${review.prNumber}`
}

async function digest(value: unknown) {
  const encoded = new TextEncoder().encode(JSON.stringify(value))
  const hash = await crypto.subtle.digest('SHA-256', encoded)
  return Array.from(new Uint8Array(hash), byte => byte.toString(16).padStart(2, '0')).join('')
}

async function ownedResult(ctx: MutationCtx, id: Id<'copilotResults'>, request: string) {
  requestId(request)
  const row = await ctx.db.get(id)
  if (!row || row.nativeRequestId !== request) throw new Error('Assistant result identity changed.')
  return row
}

async function linkedReview(ctx: MutationCtx, resultId: Id<'copilotResults'>) {
  return ctx.db
    .query('prReviewRuns')
    .withIndex('by_result', q => q.eq('resultId', resultId))
    .unique()
}

function containsControl(text: string) {
  return Array.from(text).some(character => {
    const code = character.charCodeAt(0)
    return code <= 31 || (code >= 127 && code <= 159)
  })
}

type BeginArgs = {
  requestId: string
  prompt: string
  model: string
  account: string
  review?: Review
  category?: string
  rerunOf?: Id<'copilotResults'>
}

function validateBegin(args: BeginArgs) {
  requestId(args.requestId)
  account(args.account)
  textLimit(args.prompt, 256 * 1024)
  textLimit(args.model, 256)
  if (!args.prompt.trim()) throw new Error('Enter an assistant prompt.')
  if (!args.model || containsControl(args.model)) throw new Error('Choose a valid assistant model.')
  if (args.review) reviewContext(args.review)
  validateRecord(args)
}

function validateRecord(args: BeginArgs) {
  if (args.review && args.category !== undefined && args.category !== 'pr-review')
    throw new Error('A captured pull request review is always a PR Review.')
  if (args.rerunOf !== undefined && (args.review || args.category !== undefined))
    throw new Error('A re-run keeps the saved result category and review.')
}

function fingerprintFields(args: BeginArgs) {
  const review = args.review
  const fields: unknown[] = [
    args.prompt,
    args.model,
    args.account,
    review
      ? [
          review.owner,
          review.repo,
          review.prNumber,
          review.title,
          review.head,
          review.threadStats.total,
          review.threadStats.unresolved,
          review.threadStats.outdated,
        ]
      : null,
  ]
  // Requests without the newer fields keep their original digest.
  if (args.category !== undefined || args.rerunOf !== undefined)
    fields.push([args.category ?? null, args.rerunOf ?? null])
  return fields
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

// Electron's re-run sends the saved prompt, category and metadata again; the
// metadata's ghAccount chooses the account.
async function rerunSource(ctx: MutationCtx, args: BeginArgs, sourceId: Id<'copilotResults'>) {
  const source = await ctx.db.get(sourceId)
  if (!source) throw new Error('The result to re-run no longer exists.')
  if (source.prompt !== args.prompt) throw new Error('A re-run must send the saved prompt.')
  // A native session cannot fetch a PR's diff, so its answer must never stand
  // for a review of the saved head.
  if (source.category === 'pr-review')
    throw new Error('A PR review is re-run from its pull request, not from a saved result.')
  const metadata: unknown = source.metadata
  const raw = isRecord(metadata) ? metadata.ghAccount : undefined
  // A blank or padded saved login reads as the native client reads it:
  // trimmed, and unset when empty.
  const saved = typeof raw === 'string' ? raw.trim() : ''
  if (saved !== '' && saved.toLowerCase() !== args.account.toLowerCase())
    throw new Error('Re-run this result with its saved account.')
  return {
    category: source.category,
    metadata: isRecord(metadata)
      ? { ...metadata, ghAccount: args.account }
      : { ghAccount: args.account },
    // No AI Reviews entry: a native session has no tools to fetch the PR
    // diff, so a re-run must not stand for a review of the saved head.
  }
}

async function recordFields(ctx: MutationCtx, args: BeginArgs) {
  if (args.rerunOf !== undefined) return rerunSource(ctx, args, args.rerunOf)
  const review = args.review
  if (!review)
    return {
      category: args.category ?? 'general',
      metadata: { ghAccount: args.account },
    }
  return {
    category: 'pr-review',
    metadata: {
      owner: review.owner,
      repo: review.repo,
      prNumber: review.prNumber,
      prUrl: pullRequestUrl(review),
      prTitle: review.title,
      ghAccount: args.account,
      reviewedHeadSha: review.head,
    },
  }
}

// Only a captured review creates a linked review run.
async function insertReviewRun(
  ctx: MutationCtx,
  args: BeginArgs,
  resultId: Id<'copilotResults'>,
  now: number
) {
  const review = args.review
  if (!review) return
  await ctx.db.insert('prReviewRuns', {
    owner: review.owner,
    repo: review.repo,
    prNumber: review.prNumber,
    prUrl: pullRequestUrl(review),
    prTitle: review.title,
    reviewedHeadSha: review.head,
    reviewedThreadStats: review.threadStats,
    prompt: args.prompt,
    resultId,
    model: args.model,
    ghAccount: args.account,
    status: 'pending',
    createdAt: now,
    updatedAt: now,
  })
}

// Creation, counters and optional PR snapshot commit as one transaction.
export const begin = mutation({
  args: {
    requestId: v.string(),
    prompt: v.string(),
    model: v.string(),
    account: v.string(),
    review: v.optional(reviewValidator),
    category: v.optional(categoryValidator),
    rerunOf: v.optional(v.id('copilotResults')),
  },
  handler: async (ctx, args) => {
    validateBegin(args)
    const fingerprint = await digest(fingerprintFields(args))
    const previous = await ctx.db
      .query('copilotNativeRequests')
      .withIndex('by_request', q => q.eq('requestId', args.requestId))
      .unique()
    if (previous) {
      if (previous.payloadDigest !== fingerprint)
        throw new Error('Assistant request identity cannot be reused with different content.')
      const result = await ctx.db.get(previous.resultId)
      return {
        id: previous.resultId,
        created: false,
        status: result?.status ?? ('deleted' as const),
      }
    }
    const fields = await recordFields(ctx, args)
    const now = Date.now()
    const id = await ctx.db.insert('copilotResults', {
      nativeRequestId: args.requestId,
      prompt: args.prompt,
      model: args.model,
      status: 'pending',
      createdAt: now,
      category: fields.category,
      metadata: fields.metadata,
    })
    await ctx.db.insert('copilotNativeRequests', {
      requestId: args.requestId,
      payloadDigest: fingerprint,
      resultId: id,
    })
    await insertReviewRun(ctx, args, id, now)
    await transitionResultCount(ctx, undefined, 'pending')
    return { id, created: true, status: 'pending' as const }
  },
})

// An acknowledged first claim permits one send. Unknown claims cannot be replayed.
export const claim = mutation({
  args: { id: v.id('copilotResults'), requestId: v.string() },
  handler: async (ctx, args) => {
    const row = await ownedResult(ctx, args.id, args.requestId)
    if (row.status !== 'pending') return { claimed: false, status: row.status }
    const review = await linkedReview(ctx, row._id)
    await ctx.db.patch(row._id, { status: 'running' })
    if (review) await ctx.db.patch(review._id, { status: 'running', updatedAt: Date.now() })
    await transitionResultCount(ctx, row.status, 'running')
    return { claimed: true, status: 'running' as const }
  },
})

function terminalReceipt(row: Doc<'copilotResults'>, fingerprint: string) {
  if (row.nativeFinishDigest !== fingerprint)
    throw new Error('The saved assistant outcome differs from this retry.')
  return { id: row._id, status: row.status, saved: true as const }
}

export const finish = mutation({
  args: {
    id: v.id('copilotResults'),
    requestId: v.string(),
    result: v.string(),
    failure: v.optional(failureValidator),
  },
  handler: async (ctx, args) => {
    const row = await ownedResult(ctx, args.id, args.requestId)
    textLimit(row.prompt + args.result, 768 * 1024)
    const status = args.failure ? ('failed' as const) : ('completed' as const)
    const fingerprint = await digest([status, args.result, args.failure ?? null])
    if (row.status === 'completed' || row.status === 'failed')
      return terminalReceipt(row, fingerprint)
    if (row.status !== 'running' && !args.failure)
      throw new Error('An unstarted prompt cannot complete.')
    const review = await linkedReview(ctx, row._id)
    const now = Date.now()
    await ctx.db.patch(row._id, {
      status,
      result: args.result,
      nativeFinishDigest: fingerprint,
      error: args.failure,
      completedAt: now,
      duration: now - row.createdAt,
    })
    if (review)
      await ctx.db.patch(review._id, {
        status,
        error: args.failure,
        completedAt: now,
        updatedAt: now,
      })
    await transitionResultCount(ctx, row.status, undefined)
    return { id: row._id, status, saved: true as const }
  },
})

export const reconcile = query({
  args: { requestId: v.string() },
  handler: async (ctx, args) => {
    requestId(args.requestId)
    const row = await ctx.db
      .query('copilotNativeRequests')
      .withIndex('by_request', q => q.eq('requestId', args.requestId))
      .unique()
    if (!row) return null
    const result = await ctx.db.get(row.resultId)
    return { id: row.resultId, status: result?.status ?? ('deleted' as const) }
  },
})
