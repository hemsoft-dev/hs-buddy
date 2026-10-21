import { v } from 'convex/values'
import { mutation, query } from './_generated/server'
import { MS_PER_DAY } from './lib/constants'
import { isPendingOrRunning, notFoundError, runStatusValidator } from './lib/domain'
import { projectJob } from './lib/projections'
import { deleteRun, getRunCountsByJob, patchRun } from './lib/runStore'
import {
  createRun,
  getNativeTrigger as lookupNativeTrigger,
  triggerNativeRun,
} from './lib/runCreate'
import {
  claimNativeRun,
  finalizeRun,
  getNativeClaim as lookupNativeClaim,
  publicRun,
  retireClaimPayload,
} from './lib/runClaims'

// List recent runs (last N runs)
export const listRecent = query({
  args: {
    limit: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    const limit = args.limit ?? 50

    const runs = await ctx.db.query('runs').withIndex('by_started').order('desc').take(limit)

    // Fetch job info for each run
    const runsWithJobs = await Promise.all(
      runs.map(async run => {
        const job = await ctx.db.get('jobs', run.jobId)
        const schedule = run.scheduleId ? await ctx.db.get('schedules', run.scheduleId) : null

        return {
          ...publicRun(run),
          job: projectJob(job),
          schedule: schedule
            ? {
                _id: schedule._id,
                name: schedule.name,
              }
            : null,
        }
      })
    )

    return runsWithJobs
  },
})

// List runs for a specific job
export const listByJob = query({
  args: {
    jobId: v.id('jobs'),
    limit: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    const limit = args.limit ?? 20

    return (
      await ctx.db
        .query('runs')
        .withIndex('by_job', q => q.eq('jobId', args.jobId))
        .order('desc')
        .take(limit)
    ).map(publicRun)
  },
})

// List runs for a specific schedule
export const listBySchedule = query({
  args: {
    scheduleId: v.id('schedules'),
    limit: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    const limit = args.limit ?? 20

    return (
      await ctx.db
        .query('runs')
        .withIndex('by_schedule', q => q.eq('scheduleId', args.scheduleId))
        .order('desc')
        .take(limit)
    ).map(publicRun)
  },
})

// Get single run by ID
export const get = query({
  args: { id: v.id('runs') },
  handler: async (ctx, args) => {
    const run = await ctx.db.get('runs', args.id)
    if (!run) return null

    const job = await ctx.db.get('jobs', run.jobId)
    const schedule = run.scheduleId ? await ctx.db.get('schedules', run.scheduleId) : null

    return {
      ...publicRun(run),
      job,
      schedule,
    }
  },
})

// Create new run (when starting execution)
export const create = mutation({
  args: {
    jobId: v.id('jobs'),
    scheduleId: v.optional(v.id('schedules')),
    triggeredBy: v.union(v.literal('manual'), v.literal('schedule'), v.literal('api')),
    input: v.optional(v.any()),
  },
  handler: createRun,
})

export const triggerNative = mutation({
  args: { requestId: v.string(), jobId: v.id('jobs'), input: v.optional(v.any()) },
  handler: triggerNativeRun,
})

export const getNativeTrigger = query({
  args: { requestId: v.string() },
  handler: (ctx, args) => lookupNativeTrigger(ctx, args.requestId),
})

// Update run status to running
export const markRunning = mutation({
  args: { id: v.id('runs') },
  handler: async (ctx, args) => {
    const run = await ctx.db.get('runs', args.id)
    if (!run) {
      throw notFoundError('Run', args.id)
    }

    // Only the pending -> running transition is valid. Returning for running
    // keeps dispatcher retries idempotent, while returning for terminal states
    // prevents delayed work from resurrecting an already finalized run.
    if (run.status !== 'pending') {
      return
    }

    await patchRun(ctx, args.id, {
      status: 'running',
    })
  },
})

// Complete a run successfully
export const complete = mutation({
  args: {
    id: v.id('runs'),
    output: v.optional(v.any()),
    outputFileId: v.optional(v.id('_storage')),
    claimRequestId: v.optional(v.string()),
  },
  handler: async (ctx, args) => {
    await finalizeRun(
      ctx,
      args.id,
      'completed',
      'runsCompleted',
      {
        output: args.output,
        outputFileId: args.outputFileId,
      },
      args.claimRequestId
    )
  },
})

// Fail a run
export const fail = mutation({
  args: {
    id: v.id('runs'),
    error: v.string(),
    output: v.optional(v.any()),
    claimRequestId: v.optional(v.string()),
  },
  handler: async (ctx, args) => {
    await finalizeRun(
      ctx,
      args.id,
      'failed',
      'runsFailed',
      {
        error: args.error,
        output: args.output,
      },
      args.claimRequestId
    )
  },
})

// Cancel a run
export const cancel = mutation({
  args: { id: v.id('runs') },
  handler: async (ctx, args) => {
    const run = await ctx.db.get('runs', args.id)
    if (!run) {
      throw notFoundError('Run', args.id)
    }

    if (!isPendingOrRunning(run.status)) {
      throw new Error(`Cannot cancel run with status: ${run.status}`)
    }

    const completedAt = Date.now()
    await patchRun(ctx, args.id, {
      status: 'cancelled',
      completedAt,
      duration: completedAt - run.startedAt,
    })
  },
})

// Get runs by status (for monitoring pending/running)
export const listByStatus = query({
  args: {
    status: runStatusValidator,
    limit: v.optional(v.number()),
    // Native dispatchers claim FIFO, like claimPending.
    oldestFirst: v.optional(v.boolean()),
  },
  handler: async (ctx, args) => {
    const limit = args.limit ?? 50

    return (
      await ctx.db
        .query('runs')
        .withIndex('by_status', q => q.eq('status', args.status))
        .order(args.oldestFirst ? 'asc' : 'desc')
        .take(limit)
    ).map(publicRun)
  },
})

export const claimNative = mutation({
  args: { requestId: v.string(), runId: v.id('runs') },
  handler: (ctx, args) => claimNativeRun(ctx, args.requestId, args.runId),
})

export const getNativeClaim = query({
  args: { requestId: v.string() },
  handler: (ctx, args) => lookupNativeClaim(ctx, args.requestId),
})

// Claim the oldest pending run atomically (returns run + job, or null if none pending)
export const claimPending = mutation({
  args: {},
  handler: async ctx => {
    // Get oldest pending run
    const pendingRun = await ctx.db
      .query('runs')
      .withIndex('by_status', q => q.eq('status', 'pending'))
      .order('asc')
      .first()

    if (!pendingRun) {
      return null
    }

    // Mark as running atomically
    await patchRun(ctx, pendingRun._id, {
      status: 'running',
    })

    // Fetch the associated job
    const job = await ctx.db.get('jobs', pendingRun.jobId)
    if (!job) {
      // Job was deleted â€” fail the run
      const completedAt = Date.now()
      await patchRun(ctx, pendingRun._id, {
        status: 'failed',
        error: `Job ${pendingRun.jobId} not found`,
        completedAt,
        duration: completedAt - pendingRun.startedAt,
      })
      return null
    }

    return {
      run: { ...pendingRun, status: 'running' as const },
      job,
    }
  },
})

// Cleanup old runs (keep last N days)
export const cleanup = mutation({
  args: {
    olderThanDays: v.number(),
  },
  handler: async (ctx, args) => {
    const cutoff = Date.now() - args.olderThanDays * MS_PER_DAY

    // Fetch old runs using take() to avoid unbounded .collect()
    // Filter in TypeScript code per Convex best practices (no .filter())
    const oldRuns = await ctx.db.query('runs').withIndex('by_started').order('asc').take(500)

    let deleted = 0
    for (const run of oldRuns) {
      // Only delete runs older than cutoff and not active
      if (run.startedAt >= cutoff) break
      if (!isPendingOrRunning(run.status)) {
        // react-doctor-disable-next-line react-doctor/async-await-in-loop -- Retire captured job data in the same transaction as its run history.
        await retireClaimPayload(ctx, run)
        // react-doctor-disable-next-line react-doctor/async-await-in-loop -- Cleanup walks sorted rows and stops at the cutoff boundary.
        await deleteRun(ctx, run)
        deleted++
      }
    }

    return { deleted }
  },
})

// Count runs per job (for job list badges)
const MAX_JOBS_PER_COUNT_QUERY = 100

export const countsByJob = query({
  args: { jobIds: v.array(v.id('jobs')) },
  handler: async (ctx, args) => {
    const jobIds = [...new Set(args.jobIds)]
    if (jobIds.length > MAX_JOBS_PER_COUNT_QUERY) {
      throw new Error(`Run counts can be requested for at most ${MAX_JOBS_PER_COUNT_QUERY} jobs`)
    }
    return getRunCountsByJob(ctx, jobIds)
  },
})
