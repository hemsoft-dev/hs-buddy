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

function validateBegin(args: {
  requestId: string
  prompt: string
  model: string
  account: string
  review?: Review
}) {
  requestId(args.requestId)
  account(args.account)
  textLimit(args.prompt, 256 * 1024)
  textLimit(args.model, 256)
  if (!args.prompt.trim()) throw new Error('Enter an assistant prompt.')
  if (!args.model || containsControl(args.model)) throw new Error('Choose a valid assistant model.')
  if (args.review) reviewContext(args.review)
}

// Creation, counters and optional PR snapshot commit as one transaction.
export const begin = mutation({
  args: {
    requestId: v.string(),
    prompt: v.string(),
    model: v.string(),
    account: v.string(),
    review: v.optional(reviewValidator),
  },
  handler: async (ctx, args) => {
    validateBegin(args)
    const review = args.review
    const fingerprint = await digest([
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
    ])
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
    const now = Date.now()
    const id = await ctx.db.insert('copilotResults', {
      nativeRequestId: args.requestId,
      prompt: args.prompt,
      model: args.model,
      status: 'pending',
      createdAt: now,
      category: review ? 'pr-review' : 'general',
      metadata: review
        ? {
            owner: review.owner,
            repo: review.repo,
            prNumber: review.prNumber,
            ghAccount: args.account,
            reviewedHeadSha: review.head,
          }
        : { ghAccount: args.account },
    })
    await ctx.db.insert('copilotNativeRequests', {
      requestId: args.requestId,
      payloadDigest: fingerprint,
      resultId: id,
    })
    if (review) {
      await ctx.db.insert('prReviewRuns', {
        owner: review.owner,
        repo: review.repo,
        prNumber: review.prNumber,
        prUrl: `https://github.com/${review.owner}/${review.repo}/pull/${review.prNumber}`,
        prTitle: review.title,
        prompt: args.prompt,
        resultId: id,
        model: args.model,
        ghAccount: args.account,
        reviewedHeadSha: review.head,
        reviewedThreadStats: review.threadStats,
        status: 'pending',
        createdAt: now,
        updatedAt: now,
      })
    }
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
