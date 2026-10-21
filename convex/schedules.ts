import { v } from 'convex/values'
import { internalMutation, mutation, query } from './_generated/server'
import { calculateNextRunAt, DEFAULT_TIMEZONE, validateCronExpression } from './lib/cronUtils'
import { notFoundError } from './lib/domain'
import { projectJob } from './lib/projections'
import { buildScheduleUpdateFields } from '../shared/utils/scheduleUtils'
import { continueRecoveryBatch, recoverMissedSchedule } from './lib/offlineRecovery'

/**
 * Schedule CRUD operations
 */

// List all schedules with job info
export const list = query({
  args: {},
  handler: async ctx => {
    const schedules = await ctx.db.query('schedules').collect()

    // Fetch associated jobs
    const schedulesWithJobs = await Promise.all(
      schedules.map(async schedule => {
        const job = await ctx.db.get('jobs', schedule.jobId)
        return {
          ...schedule,
          job: projectJob(job),
        }
      })
    )

    return schedulesWithJobs
  },
})

// List only enabled schedules (for scheduler)
export const listEnabled = query({
  args: {},
  handler: async ctx => {
    return await ctx.db
      .query('schedules')
      .withIndex('by_enabled', q => q.eq('enabled', true))
      .collect()
  },
})

// Get single schedule by ID
export const get = query({
  args: { id: v.id('schedules') },
  handler: async (ctx, args) => {
    const schedule = await ctx.db.get('schedules', args.id)
    if (!schedule) return null

    const job = await ctx.db.get('jobs', schedule.jobId)
    return {
      ...schedule,
      job: projectJob(job, true),
    }
  },
})

// Create new schedule
export const create = mutation({
  args: {
    jobId: v.id('jobs'),
    name: v.string(),
    description: v.optional(v.string()),
    cron: v.string(),
    timezone: v.optional(v.string()),
    enabled: v.boolean(),
    params: v.optional(v.any()),
    missedPolicy: v.union(v.literal('catchup'), v.literal('skip'), v.literal('last')),
  },
  handler: async (ctx, args) => {
    // Verify job exists
    const job = await ctx.db.get('jobs', args.jobId)
    if (!job) {
      throw notFoundError('Job', args.jobId)
    }

    const now = Date.now()
    const timezone = args.timezone ?? DEFAULT_TIMEZONE
    // A paused schedule must still be enableable later.
    validateCronExpression(args.cron, timezone)

    // Calculate nextRunAt if schedule is enabled
    const nextRunAt = args.enabled ? calculateNextRunAt(args.cron, timezone) : undefined

    const id = await ctx.db.insert('schedules', {
      jobId: args.jobId,
      name: args.name,
      description: args.description,
      cron: args.cron,
      timezone,
      enabled: args.enabled,
      params: args.params,
      missedPolicy: args.missedPolicy,
      nextRunAt,
      createdAt: now,
      updatedAt: now,
    })

    return id
  },
})

// Update existing schedule
export const update = mutation({
  args: {
    id: v.id('schedules'),
    name: v.optional(v.string()),
    description: v.optional(v.string()),
    cron: v.optional(v.string()),
    timezone: v.optional(v.string()),
    enabled: v.optional(v.boolean()),
    params: v.optional(v.any()),
    missedPolicy: v.optional(v.union(v.literal('catchup'), v.literal('skip'), v.literal('last'))),
  },
  handler: async (ctx, args) => {
    const { id, ...updates } = args

    const existing = await ctx.db.get('schedules', id)
    if (!existing) {
      throw notFoundError('Schedule', id)
    }
    if (updates.cron !== undefined || updates.timezone !== undefined) {
      // Validate even while paused, so enabling it later cannot fail.
      validateCronExpression(
        updates.cron ?? existing.cron,
        updates.timezone ?? existing.timezone ?? DEFAULT_TIMEZONE
      )
    }

    const now = Date.now()
    const updateData = buildScheduleUpdateFields(
      updates,
      existing,
      now,
      DEFAULT_TIMEZONE,
      calculateNextRunAt
    )

    await ctx.db.patch('schedules', id, updateData)
    return id
  },
})

// Delete schedule
export const remove = mutation({
  args: { id: v.id('schedules') },
  handler: async (ctx, args) => {
    const existing = await ctx.db.get('schedules', args.id)
    if (!existing) {
      throw notFoundError('Schedule', args.id)
    }

    await ctx.db.delete('schedules', args.id)
    return args.id
  },
})

// Toggle schedule enabled/disabled
export const toggle = mutation({
  args: { id: v.id('schedules') },
  handler: async (ctx, args) => {
    const schedule = await ctx.db.get('schedules', args.id)
    if (!schedule) {
      throw notFoundError('Schedule', args.id)
    }

    const newEnabled = !schedule.enabled
    const now = Date.now()

    // Calculate nextRunAt if being enabled
    const nextRunAt = newEnabled
      ? calculateNextRunAt(schedule.cron, schedule.timezone ?? DEFAULT_TIMEZONE)
      : undefined

    await ctx.db.patch('schedules', args.id, {
      enabled: newEnabled,
      nextRunAt,
      updatedAt: now,
    })

    return newEnabled
  },
})

// Recovery rereads the cursor and commits all missed work in one transaction.
export const recoverMissed = mutation({
  args: { id: v.id('schedules') },
  handler: (ctx, args) => recoverMissedSchedule(ctx, args.id),
})

// Convex durably schedules each continuation in the preceding batch transaction.
export const continueRecovery = internalMutation({
  args: {
    scheduleId: v.id('schedules'),
    jobId: v.id('jobs'),
    input: v.optional(v.any()),
    remaining: v.number(),
    startedAt: v.number(),
  },
  handler: continueRecoveryBatch,
})

// Legacy cursor update retained for API compatibility, not used by recovery.
export const advanceNextRun = mutation({
  args: {
    id: v.id('schedules'),
    nextRunAt: v.number(),
    lastRunAt: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    const schedule = await ctx.db.get('schedules', args.id)
    if (!schedule) {
      throw notFoundError('Schedule', args.id)
    }

    const patch: Record<string, unknown> = {
      nextRunAt: args.nextRunAt,
      updatedAt: Date.now(),
    }

    if (args.lastRunAt !== undefined) {
      patch.lastRunAt = args.lastRunAt
    }

    await ctx.db.patch('schedules', args.id, patch)
    return args.id
  },
})
