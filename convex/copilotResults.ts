import { v } from 'convex/values'
import type { Id } from './_generated/dataModel'
import { mutation, query, type MutationCtx } from './_generated/server'
import { MS_PER_DAY } from './lib/constants'
import { copilotResultStatusValidator, isPendingOrRunning, notFoundError } from './lib/domain'
import { transitionResultCount } from './lib/copilotResultCounts'

/**
 * Copilot SDK Results — CRUD operations for captured Copilot prompt results.
 */

async function deleteResultWithReviewRuns(ctx: MutationCtx, resultId: Id<'copilotResults'>) {
  const reviewRuns = await ctx.db
    .query('prReviewRuns')
    .withIndex('by_result', q => q.eq('resultId', resultId))
    .collect()

  for (const reviewRun of reviewRuns) {
    // react-doctor-disable-next-line react-doctor/async-await-in-loop -- Every linked row must be removed before its result.
    await ctx.db.delete(reviewRun._id)
  }
  await ctx.db.delete(resultId)
}

// List recent results (newest first)
export const listRecent = query({
  args: {
    limit: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    const limit = args.limit ?? 50
    return ctx.db.query('copilotResults').withIndex('by_created').order('desc').take(limit)
  },
})

// List results by category
export const listByCategory = query({
  args: {
    category: v.string(),
    limit: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    const limit = args.limit ?? 50
    return ctx.db
      .query('copilotResults')
      .withIndex('by_category', q => q.eq('category', args.category))
      .order('desc')
      .take(limit)
  },
})

// List results by status
export const listByStatus = query({
  args: {
    status: copilotResultStatusValidator,
    limit: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    const limit = args.limit ?? 50
    return ctx.db
      .query('copilotResults')
      .withIndex('by_status', q => q.eq('status', args.status))
      .order('desc')
      .take(limit)
  },
})

// Get single result by ID
export const get = query({
  args: { id: v.id('copilotResults') },
  handler: async (ctx, args) => {
    return ctx.db.get(args.id)
  },
})

// Count pending/running results (for badges)
export const countActive = query({
  args: {},
  handler: async ctx => {
    const counts = await ctx.db
      .query('copilotResultCounts')
      .withIndex('by_key', q => q.eq('key', 'default'))
      .unique()
    return { pending: counts?.pending ?? 0, running: counts?.running ?? 0 }
  },
})

// Create a new pending result (called when user initiates a prompt)
export const create = mutation({
  args: {
    prompt: v.string(),
    category: v.optional(v.string()),
    metadata: v.optional(v.any()),
  },
  handler: async (ctx, args) => {
    const id = await ctx.db.insert('copilotResults', {
      prompt: args.prompt,
      status: 'pending',
      category: args.category,
      metadata: args.metadata,
      createdAt: Date.now(),
    })
    await transitionResultCount(ctx, undefined, 'pending')
    return id
  },
})

// Mark a result as running
export const markRunning = mutation({
  args: {
    id: v.id('copilotResults'),
    model: v.optional(v.string()),
  },
  handler: async (ctx, args) => {
    const doc = await ctx.db.get(args.id)
    if (!doc) throw notFoundError('CopilotResult', args.id)

    await transitionResultCount(ctx, doc.status, 'running')
    await ctx.db.patch(args.id, {
      status: 'running',
      model: args.model,
    })
  },
})

// Complete a result with markdown output
export const complete = mutation({
  args: {
    id: v.id('copilotResults'),
    result: v.string(),
    model: v.optional(v.string()),
  },
  handler: async (ctx, args) => {
    const doc = await ctx.db.get(args.id)
    if (!doc) throw notFoundError('CopilotResult', args.id)

    const completedAt = Date.now()
    await transitionResultCount(ctx, doc.status, undefined)
    await ctx.db.patch(args.id, {
      status: 'completed',
      result: args.result,
      model: args.model,
      completedAt,
      duration: completedAt - doc.createdAt,
    })
  },
})

// Fail a result
export const fail = mutation({
  args: {
    id: v.id('copilotResults'),
    error: v.string(),
  },
  handler: async (ctx, args) => {
    const doc = await ctx.db.get(args.id)
    if (!doc) throw notFoundError('CopilotResult', args.id)

    const completedAt = Date.now()
    await transitionResultCount(ctx, doc.status, undefined)
    await ctx.db.patch(args.id, {
      status: 'failed',
      error: args.error,
      completedAt,
      duration: completedAt - doc.createdAt,
    })
  },
})

// Delete a result
export const remove = mutation({
  args: { id: v.id('copilotResults') },
  handler: async (ctx, args) => {
    const doc = await ctx.db.get(args.id)
    if (doc) {
      await transitionResultCount(ctx, doc.status, undefined)
    }
    await deleteResultWithReviewRuns(ctx, args.id)
  },
})

// Cleanup old results (keep last N days)
export const cleanup = mutation({
  args: {
    olderThanDays: v.number(),
  },
  handler: async (ctx, args) => {
    const cutoff = Date.now() - args.olderThanDays * MS_PER_DAY
    const oldResults = await ctx.db
      .query('copilotResults')
      .withIndex('by_created')
      .order('asc')
      .take(500)

    let deleted = 0
    for (const result of oldResults) {
      if (result.createdAt >= cutoff) break
      if (!isPendingOrRunning(result.status)) {
        // react-doctor-disable-next-line react-doctor/async-await-in-loop -- Cleanup walks sorted rows and stops at the cutoff boundary.
        await deleteResultWithReviewRuns(ctx, result._id)
        deleted++
      }
    }
    return { deleted }
  },
})
