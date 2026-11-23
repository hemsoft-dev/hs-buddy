import { v } from 'convex/values'
import type { Doc } from './_generated/dataModel'
import { mutation, query } from './_generated/server'
import { MS_PER_DAY } from './lib/constants'
import { isPendingOrRunning, notFoundError, runStatusValidator } from './lib/domain'
import { projectJob } from './lib/projections'
import { deleteRun, getRunCountsByJob, patchRun } from './lib/runStore'
import {
  createRun,
  getNativeTrigger as lookupNativeTrigger,
  triggerNativeRun,
  validateRequestId,
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

const NATIVE_CLEANUP_BATCH = 500
// Convex stops a transaction that reads more than 16 MiB. Runs can carry
// large inline output, so one request stops reading at half of that.
const NATIVE_CLEANUP_READ_BUDGET = 8 * 1024 * 1024
const FINISHED_RUN_STATUSES = ['completed', 'failed', 'cancelled'] as const
// A receipt only has to outlive the client's resend after a reconnect.
const CLEANUP_RECEIPT_RETENTION_MS = MS_PER_DAY
const CLEANUP_RECEIPT_PRUNE = 50

// UTF-8 bytes of the document's JSON, plus its binary values and int64s,
// which JSON cannot carry. Convex counts stored bytes, not UTF-16 units.
function approximateBytes(run: Doc<'runs'>) {
  let binary = 0
  const text = JSON.stringify(run, (_key: string, value: unknown) => {
    if (value instanceof ArrayBuffer) {
      binary += value.byteLength
      return null
    }
    if (typeof value === 'bigint') {
      binary += 8
      return null
    }
    return value
  })
  return new TextEncoder().encode(text).byteLength + binary
}

// Native cleanup: the same deletion as `cleanup`, once per request identity.
// It reads finished runs older than the cutoff through the status index, so
// pending and running runs never use up the batch or stall it. `more` means
// it stopped at the batch or read budget with another old run still waiting.
export const cleanupNative = mutation({
  args: {
    requestId: v.string(),
    olderThanDays: v.number(),
  },
  handler: async (ctx, args) => {
    validateRequestId(args.requestId)
    const previous = await ctx.db
      .query('automationCleanupReceipts')
      .withIndex('by_request', q => q.eq('requestId', args.requestId))
      .unique()
    if (previous) {
      if (previous.olderThanDays !== args.olderThanDays)
        throw new Error('Cleanup request identity cannot be reused with a different age.')
      return { deleted: previous.deleted, more: previous.more }
    }
    const now = Date.now()
    const cutoff = now - args.olderThanDays * MS_PER_DAY
    const finished: Doc<'runs'>[] = []
    let read = 0
    let more = false
    scan: for (const status of FINISHED_RUN_STATUSES) {
      for await (const run of ctx.db
        .query('runs')
        .withIndex('by_status_started', q => q.eq('status', status).lt('startedAt', cutoff))
        .order('asc')) {
        if (finished.length === NATIVE_CLEANUP_BATCH || read >= NATIVE_CLEANUP_READ_BUDGET) {
          more = true
          break scan
        }
        read += approximateBytes(run)
        finished.push(run)
      }
    }
    for (const run of finished) {
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- Retire captured job data in the same transaction as its run history.
      await retireClaimPayload(ctx, run)
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- Deletes stay in index order within one bounded batch.
      await deleteRun(ctx, run)
    }
    const deleted = finished.length
    // An empty cleanup changed nothing, so a resend may simply run again.
    if (deleted > 0) {
      await ctx.db.insert('automationCleanupReceipts', {
        requestId: args.requestId,
        olderThanDays: args.olderThanDays,
        deleted,
        more,
        createdAt: now,
      })
    }
    const expired = await ctx.db
      .query('automationCleanupReceipts')
      .withIndex('by_created', q => q.lt('createdAt', now - CLEANUP_RECEIPT_RETENTION_MS))
      .take(CLEANUP_RECEIPT_PRUNE)
    for (const receipt of expired) {
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- A small, bounded prune of expired receipts.
      await ctx.db.delete(receipt._id)
    }
    return { deleted, more }
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
