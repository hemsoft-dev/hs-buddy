import { paginationOptsValidator, type PaginationOptions } from 'convex/server'
import { v } from 'convex/values'
import type { Doc } from './_generated/dataModel'
import { query } from './_generated/server'
import { copilotResultStatusValidator } from './lib/domain'

// Lists return display summaries; callers fetch existing full records on demand.
function pageOptions(options: PaginationOptions): PaginationOptions {
  if (!Number.isInteger(options.numItems) || options.numItems < 1 || options.numItems > 25) {
    throw new Error('Choose a page size from 1 to 25.')
  }
  return { ...options, maximumRowsRead: 25, maximumBytesRead: 2 * 1024 * 1024 }
}

// Native Chat saves its whole conversation: a preamble and earlier turns,
// then the turn that ran. Its preview names that last user turn. Electron
// prompts and captured reviews keep their whole prompt, and so does a turn
// that would leave nothing to show.
function promptForPreview(row: Doc<'copilotResults'>): string {
  if (row.nativeRequestId === undefined || row.category === 'pr-review') return row.prompt
  const marker = '\nUser: '
  const index = row.prompt.lastIndexOf(marker)
  if (index < 0) return row.prompt
  const turn = row.prompt.slice(index + marker.length)
  return turn.trim() === '' ? row.prompt : turn
}

function preview(text: string): string {
  let output = ''
  let count = 0
  for (const character of text) {
    if (count === 200) return output + '…'
    const code = character.charCodeAt(0)
    output += code <= 32 || (code >= 127 && code <= 159) ? ' ' : character
    count += 1
  }
  return output
}

// Electron opens metadata.prUrl only for pr-review results; the list needs it to offer that link.
function reviewedPullRequestUrl(row: Doc<'copilotResults'>): string | undefined {
  if (row.category !== 'pr-review') return
  const metadata: unknown = row.metadata
  if (typeof metadata !== 'object' || metadata === null || !('prUrl' in metadata)) return
  const url = metadata.prUrl
  return typeof url === 'string' && url.length <= 2048 ? url : undefined
}

// Electron's sidebar labels a pr-review result with metadata.prTitle.
function reviewedPullRequestTitle(row: Doc<'copilotResults'>): string | undefined {
  if (row.category !== 'pr-review') return
  const metadata: unknown = row.metadata
  if (typeof metadata !== 'object' || metadata === null || !('prTitle' in metadata)) return
  const title = metadata.prTitle
  return typeof title === 'string' ? preview(title) : undefined
}

function resultSummary(row: Doc<'copilotResults'>) {
  return {
    _id: row._id,
    promptPreview: preview(promptForPreview(row)),
    status: row.status,
    modelLabel: row.model === undefined ? undefined : preview(row.model),
    categoryLabel: row.category === undefined ? undefined : preview(row.category),
    createdAt: row.createdAt,
    completedAt: row.completedAt,
    duration: row.duration,
    hasOutput: row.result !== undefined,
    prUrl: reviewedPullRequestUrl(row),
    prTitle: reviewedPullRequestTitle(row),
  }
}

function capturedHead(value: string | undefined) {
  return value !== undefined && /^(?:[a-f\d]{40}|[a-f\d]{64})$/i.test(value) ? value : undefined
}

export const listUsage = query({
  args: {
    paginationOpts: paginationOptsValidator,
    accountUsername: v.string(),
    org: v.string(),
    billingYear: v.number(),
    billingMonth: v.number(),
  },
  handler: async (ctx, args) => {
    const result = await ctx.db
      .query('copilotUsageHistory')
      .withIndex('by_account_period', q =>
        q
          .eq('accountUsername', args.accountUsername)
          .eq('org', args.org)
          .eq('billingYear', args.billingYear)
          .eq('billingMonth', args.billingMonth)
      )
      .order('desc')
      .paginate(pageOptions(args.paginationOpts))
    return result
  },
})

export const listResults = query({
  args: { paginationOpts: paginationOptsValidator },
  handler: async (ctx, args) => {
    const result = await ctx.db
      .query('copilotResults')
      .withIndex('by_created')
      .order('desc')
      .paginate(pageOptions(args.paginationOpts))
    return { ...result, page: result.page.map(resultSummary) }
  },
})

export const listResultsByStatus = query({
  args: { paginationOpts: paginationOptsValidator, status: copilotResultStatusValidator },
  handler: async (ctx, args) => {
    const result = await ctx.db
      .query('copilotResults')
      .withIndex('by_status', q => q.eq('status', args.status))
      .order('desc')
      .paginate(pageOptions(args.paginationOpts))
    return { ...result, page: result.page.map(resultSummary) }
  },
})

export const listResultsByCategory = query({
  args: { paginationOpts: paginationOptsValidator, category: v.string() },
  handler: async (ctx, args) => {
    const result = await ctx.db
      .query('copilotResults')
      .withIndex('by_category', q => q.eq('category', args.category))
      .order('desc')
      .paginate(pageOptions(args.paginationOpts))
    return { ...result, page: result.page.map(resultSummary) }
  },
})

export const listReviews = query({
  args: {
    paginationOpts: paginationOptsValidator,
    owner: v.string(),
    repo: v.string(),
    prNumber: v.number(),
  },
  handler: async (ctx, args) => {
    const result = await ctx.db
      .query('prReviewRuns')
      .withIndex('by_pr', q =>
        q.eq('owner', args.owner).eq('repo', args.repo).eq('prNumber', args.prNumber)
      )
      .order('desc')
      .paginate(pageOptions(args.paginationOpts))
    return {
      ...result,
      page: result.page.map(row => ({
        _id: row._id,
        resultId: row.resultId,
        titlePreview: preview(row.prTitle),
        status: row.status,
        modelLabel: row.model === undefined ? undefined : preview(row.model),
        accountLabel: row.ghAccount === undefined ? undefined : preview(row.ghAccount),
        reviewedHeadSha: capturedHead(row.reviewedHeadSha),
        reviewedThreadStats: row.reviewedThreadStats,
        createdAt: row.createdAt,
        updatedAt: row.updatedAt,
        completedAt: row.completedAt,
      })),
    }
  },
})
