import { v } from 'convex/values'
import { mutation, query, type DatabaseReader, type DatabaseWriter } from './_generated/server'
import { MS_PER_DAY } from './lib/constants'
import { SINGLETON_KEY } from './lib/domain'
import { validateRequestId } from './lib/runCreate'
import {
  validateStatFields,
  buildIncrementPatch,
  buildInitialStatsDoc,
} from '../shared/utils/statsMutationUtils'

/**
 * Buddy Stats — centralized usage statistics (singleton pattern)
 *
 * All counters are stored in a single document keyed by "default".
 * Multiple clients (desktop, mobile, web) share the same stats.
 */

/** Zero-value defaults for all stat fields */
const DEFAULT_STATS = {
  appLaunches: 0,
  tabsOpened: 0,
  prsViewed: 0,
  prsReviewed: 0,
  prsMergedWatched: 0,
  reposBrowsed: 0,
  repoDetailViews: 0,
  jobsCreated: 0,
  runsTriggered: 0,
  runsCompleted: 0,
  runsFailed: 0,
  schedulesCreated: 0,
  bookmarksCreated: 0,
  settingsChanged: 0,
  searchesPerformed: 0,
  copilotPrReviews: 0,
  firstLaunchDate: 0,
  totalUptimeMs: 0,
}

/** Valid counter field names for increment/batchIncrement */
const COUNTER_FIELDS = new Set([
  'appLaunches',
  'tabsOpened',
  'prsViewed',
  'prsReviewed',
  'prsMergedWatched',
  'reposBrowsed',
  'repoDetailViews',
  'jobsCreated',
  'runsTriggered',
  'runsCompleted',
  'runsFailed',
  'schedulesCreated',
  'bookmarksCreated',
  'settingsChanged',
  'searchesPerformed',
  'copilotPrReviews',
])

async function getDefaultStats(db: DatabaseReader) {
  return db
    .query('buddyStats')
    .withIndex('by_key', q => q.eq('key', SINGLETON_KEY))
    .first()
}

// ── Queries ───────────────────────────────────────────────────────────────

/**
 * Get the buddy stats document (returns defaults if not yet created)
 */
export const get = query({
  args: {},
  handler: async ctx => {
    const doc = await getDefaultStats(ctx.db)

    if (doc) return doc

    // Return in-memory defaults (not persisted) until first mutation
    return {
      key: SINGLETON_KEY,
      ...DEFAULT_STATS,
      lastSessionStart: undefined,
      createdAt: 0,
      updatedAt: 0,
    }
  },
})

// ── Mutations ─────────────────────────────────────────────────────────────

/**
 * Increment a single counter field by a given amount (default 1).
 */
export const increment = mutation({
  args: {
    field: v.string(),
    amount: v.optional(v.number()),
  },
  handler: async (ctx, { field, amount }) => {
    if (!COUNTER_FIELDS.has(field)) {
      throw new Error(`Invalid stat field: ${field}`)
    }

    const amt = amount ?? 1
    const now = Date.now()

    const existing = await getDefaultStats(ctx.db)

    if (existing) {
      const current = ((existing as Record<string, unknown>)[field] as number) ?? 0
      await ctx.db.patch('buddyStats', existing._id, {
        [field]: current + amt,
        updatedAt: now,
      })
      return existing._id
    }

    // First-ever stat write — create the document
    return await ctx.db.insert('buddyStats', {
      key: SINGLETON_KEY,
      ...DEFAULT_STATS,
      [field]: amt,
      firstLaunchDate: now,
      createdAt: now,
      updatedAt: now,
    })
  },
})

/**
 * Increment multiple counter fields in a single transaction.
 * Accepts a record of { fieldName: incrementAmount }.
 */
export const batchIncrement = mutation({
  args: {
    fields: v.any(), // Record<string, number>
  },
  handler: async (ctx, { fields }) => applyIncrement(ctx.db, fields as Record<string, number>),
})

async function applyIncrement(db: DatabaseWriter, fields: Record<string, number>) {
  const entries = Object.entries(fields)
  validateStatFields(fields, COUNTER_FIELDS)

  const now = Date.now()

  const existing = await getDefaultStats(db)

  if (existing) {
    const patch = buildIncrementPatch(entries, existing as Record<string, unknown>)
    patch.updatedAt = now
    await db.patch('buddyStats', existing._id, patch)
    return existing._id
  }

  const doc = buildInitialStatsDoc({ key: SINGLETON_KEY, ...DEFAULT_STATS }, entries, now)
  return await db.insert('buddyStats', doc as never)
}

// A receipt only has to outlive the client's resend after a reconnect.
const INCREMENT_RECEIPT_RETENTION_MS = MS_PER_DAY
const INCREMENT_RECEIPT_PRUNE = 50

/**
 * Native counters: the same validation and increment as `batchIncrement`,
 * once per request identity. The Rust Convex client resends an unacknowledged
 * mutation under a new session after a reconnect, so a repeated `requestId`
 * returns without counting again.
 */
export const incrementNative = mutation({
  args: {
    requestId: v.string(),
    fields: v.any(), // Record<string, number>
  },
  handler: async (ctx, { requestId, fields }) => {
    validateRequestId(requestId)
    const previous = await ctx.db
      .query('buddyStatsReceipts')
      .withIndex('by_request', q => q.eq('requestId', requestId))
      .unique()
    if (previous) return { counted: false }
    await applyIncrement(ctx.db, fields as Record<string, number>)
    const now = Date.now()
    await ctx.db.insert('buddyStatsReceipts', { requestId, createdAt: now })
    const expired = await ctx.db
      .query('buddyStatsReceipts')
      .withIndex('by_created', q => q.lt('createdAt', now - INCREMENT_RECEIPT_RETENTION_MS))
      .take(INCREMENT_RECEIPT_PRUNE)
    for (const receipt of expired) {
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- A small, bounded prune of expired receipts.
      await ctx.db.delete(receipt._id)
    }
    return { counted: true }
  },
})

/**
 * Record a new session start.
 * Idempotent: if lastSessionStart is already set (e.g., React strict mode
 * double-mount), this is a no-op to avoid double-counting appLaunches
 * and resetting the session timer.
 */
export const recordSessionStart = mutation({
  args: {},
  handler: async ctx => (await startSession(ctx.db)).id,
})

async function startSession(db: DatabaseWriter, owner?: string) {
  const now = Date.now()

  const existing = await getDefaultStats(db)

  if (existing) {
    // If a session is already active, don't double-count
    if (existing.lastSessionStart) {
      return { id: existing._id, started: false }
    }

    const patch: Record<string, unknown> = {
      appLaunches: existing.appLaunches + 1,
      lastSessionStart: now,
      updatedAt: now,
    }
    // Set firstLaunchDate if it was never set (or zero)
    if (!existing.firstLaunchDate) {
      patch.firstLaunchDate = now
    }
    // An Electron start (no owner) clears any owner left by an older session.
    patch.sessionOwner = owner
    await db.patch('buddyStats', existing._id, patch)
    return { id: existing._id, started: true }
  }

  const id = await db.insert('buddyStats', {
    key: SINGLETON_KEY,
    ...DEFAULT_STATS,
    appLaunches: 1,
    firstLaunchDate: now,
    lastSessionStart: now,
    ...(owner ? { sessionOwner: owner } : {}),
    createdAt: now,
    updatedAt: now,
  })
  return { id, started: true }
}

/**
 * Record session end — flush accumulated uptime.
 * Reads lastSessionStart, computes elapsed, adds to totalUptimeMs, clears lastSessionStart.
 */
export const recordSessionEnd = mutation({
  args: {},
  handler: async ctx => {
    await flushUptime(ctx.db, { end: true })
  },
})

/**
 * Add the time since `lastSessionStart` to `totalUptimeMs`, then end the
 * session (and its owner) or restart its clock. With `owner`, only that
 * client's session.
 */
async function flushUptime(db: DatabaseWriter, { end, owner }: { end: boolean; owner?: string }) {
  const now = Date.now()

  const existing = await getDefaultStats(db)

  if (!existing || !existing.lastSessionStart) return
  if (owner !== undefined && existing.sessionOwner !== owner) return

  const elapsed = Math.max(0, now - existing.lastSessionStart)

  await db.patch('buddyStats', existing._id, {
    totalUptimeMs: existing.totalUptimeMs + elapsed,
    lastSessionStart: end ? undefined : now,
    ...(end ? { sessionOwner: undefined } : {}),
    updatedAt: now,
  })
}

/**
 * Checkpoint uptime without ending the session.
 * Flushes accumulated time and resets lastSessionStart to now.
 * Used as a periodic heartbeat (every 5 min) to guard against crashes.
 */
export const checkpointUptime = mutation({
  args: {},
  handler: async ctx => {
    await flushUptime(ctx.db, { end: false })
  },
})

// ── Native session ────────────────────────────────────────────────────────
//
// The native app names its session with one UUID per launch (`owner`), so a
// resent start is recognised and a native checkpoint or quit never touches a
// session another client opened. Electron keeps the functions above.

/**
 * Start the native session. `started` is true when this owner holds the
 * session: newly opened (counting a launch), or already its own (a resend).
 * Another client's open session is left as it is and `started` is false.
 */
export const recordSessionStartNative = mutation({
  args: { owner: v.string() },
  handler: async (ctx, { owner }) => {
    validateRequestId(owner)
    const existing = await getDefaultStats(ctx.db)
    if (existing?.lastSessionStart) return { started: existing.sessionOwner === owner }
    const { started } = await startSession(ctx.db, owner)
    return { started }
  },
})

/** `checkpointUptime`, for the owner's own session only. */
export const checkpointUptimeNative = mutation({
  args: { owner: v.string() },
  handler: async (ctx, { owner }) => {
    validateRequestId(owner)
    await flushUptime(ctx.db, { end: false, owner })
  },
})

/** `recordSessionEnd`, for the owner's own session only; it clears the owner. */
export const recordSessionEndNative = mutation({
  args: { owner: v.string() },
  handler: async (ctx, { owner }) => {
    validateRequestId(owner)
    await flushUptime(ctx.db, { end: true, owner })
  },
})
