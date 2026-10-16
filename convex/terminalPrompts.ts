import { v } from 'convex/values'
import { mutation, query } from './_generated/server'
import { notFoundError } from './lib/domain'
import type { Infer } from 'convex/values'
import type { MutationCtx } from './_generated/server'
import type { Id } from './_generated/dataModel'

function normalizeContent(content: string): string {
  return content.replace(/\r\n/g, '\n')
}

function promptRevision(value: number | undefined): number {
  const revision = value ?? 0
  if (!Number.isSafeInteger(revision) || revision < 0 || revision >= Number.MAX_SAFE_INTEGER)
    throw new Error('Prompt revision is invalid or exhausted')
  return revision
}

function validatePromptFields(fields: { title?: string; content?: string }) {
  if (fields.title !== undefined && !fields.title.trim()) {
    throw new Error('Title is required')
  }

  if (fields.content !== undefined && !normalizeContent(fields.content).trim()) {
    throw new Error('Prompt content is required')
  }
}

type PromptSortable = {
  lastUsedAt?: number
  updatedAt: number
  title: string
  sortOrder?: number
}

function compareByLastUsed(a: PromptSortable, b: PromptSortable): number {
  return (b.lastUsedAt ?? 0) - (a.lastUsedAt ?? 0)
}

function compareBySortOrder(a: PromptSortable, b: PromptSortable): number {
  return (a.sortOrder ?? Number.MAX_SAFE_INTEGER) - (b.sortOrder ?? Number.MAX_SAFE_INTEGER)
}

function comparePrompts(a: PromptSortable, b: PromptSortable) {
  const lastUsedDiff = compareByLastUsed(a, b)
  if (lastUsedDiff !== 0) return lastUsedDiff

  const sortOrderDiff = compareBySortOrder(a, b)
  if (sortOrderDiff !== 0) return sortOrderDiff

  const updatedAtDiff = b.updatedAt - a.updatedAt
  if (updatedAtDiff !== 0) return updatedAtDiff

  return a.title.localeCompare(b.title)
}

function buildPromptPatch(args: { title?: string; content?: string; sortOrder?: number }): {
  title?: string
  content?: string
  sortOrder?: number
  updatedAt?: number
} {
  const patch: {
    title?: string
    content?: string
    sortOrder?: number
  } = {}

  if (args.title !== undefined) patch.title = args.title.trim()
  if (args.content !== undefined) patch.content = normalizeContent(args.content)
  if (args.sortOrder !== undefined) patch.sortOrder = args.sortOrder

  return patch
}

export const list = query({
  args: {},
  handler: async ctx => {
    const prompts = await ctx.db.query('terminalPrompts').collect()
    return prompts.sort(comparePrompts)
  },
})

export const create = mutation({
  args: {
    title: v.string(),
    content: v.string(),
    sortOrder: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    validatePromptFields(args)

    const now = Date.now()
    const allPrompts = await ctx.db.query('terminalPrompts').collect()
    const sortOrder = args.sortOrder ?? allPrompts.length

    return await ctx.db.insert('terminalPrompts', {
      title: args.title.trim(),
      content: normalizeContent(args.content),
      sortOrder,
      createdAt: now,
      updatedAt: now,
    })
  },
})

export const update = mutation({
  args: {
    id: v.id('terminalPrompts'),
    title: v.optional(v.string()),
    content: v.optional(v.string()),
    sortOrder: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    const existing = await ctx.db.get(args.id)
    if (!existing) {
      throw notFoundError('Terminal prompt', args.id)
    }

    validatePromptFields(args)

    const patch = buildPromptPatch(args)

    if (Object.keys(patch).length === 0) {
      return args.id
    }

    patch.updatedAt = Date.now()
    await ctx.db.patch(args.id, { ...patch, revision: promptRevision(existing.revision) + 1 })
    return args.id
  },
})

export const markUsed = mutation({
  args: { id: v.id('terminalPrompts') },
  handler: async (ctx, args) => {
    const existing = await ctx.db.get(args.id)
    if (!existing) {
      return args.id
    }

    const now = Date.now()
    await ctx.db.patch(args.id, {
      lastUsedAt: now,
      updatedAt: now,
    })
    return args.id
  },
})

export const remove = mutation({
  args: { id: v.id('terminalPrompts') },
  handler: async (ctx, args) => {
    const existing = await ctx.db.get(args.id)
    if (!existing) {
      return args.id
    }

    await ctx.db.delete(args.id)
    return args.id
  },
})

const nativeOperation = v.union(
  v.object({
    kind: v.literal('create'),
    title: v.string(),
    content: v.string(),
    sortOrder: v.optional(v.number()),
  }),
  v.object({
    kind: v.literal('update'),
    id: v.id('terminalPrompts'),
    expectedRevision: v.number(),
    title: v.string(),
    content: v.string(),
    sortOrder: v.optional(v.number()),
  }),
  v.object({
    kind: v.literal('remove'),
    id: v.id('terminalPrompts'),
    expectedRevision: v.number(),
  })
)
type NativeOperation = Infer<typeof nativeOperation>
type NativeResult = {
  status: 'saved' | 'deleted' | 'conflict'
  id: Id<'terminalPrompts'>
  revision: number
}

async function operationDigest(operation: NativeOperation): Promise<string> {
  if (
    'sortOrder' in operation &&
    operation.sortOrder !== undefined &&
    !Number.isFinite(operation.sortOrder)
  )
    throw new Error('Native prompt sort order must be finite')
  // This flat, validator-owned payload has a stable key order and retains the
  // exact supplied text. Reusing an ID with even a differently spaced edit fails.
  const bytes = new TextEncoder().encode(
    JSON.stringify(
      Object.fromEntries(Object.entries(operation).sort(([a], [b]) => a.localeCompare(b)))
    )
  )
  if (bytes.byteLength > 128 * 1024) throw new Error('Native prompt exceeds 128 KiB')
  const digest = await crypto.subtle.digest('SHA-256', bytes)
  return Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, '0')).join('')
}

async function createNative(
  ctx: MutationCtx,
  operation: Extract<NativeOperation, { kind: 'create' }>
): Promise<NativeResult> {
  validatePromptFields(operation)
  const now = Date.now()
  const prompts = await ctx.db.query('terminalPrompts').collect()
  const id = await ctx.db.insert('terminalPrompts', {
    title: operation.title.trim(),
    content: normalizeContent(operation.content),
    sortOrder: operation.sortOrder ?? prompts.length,
    createdAt: now,
    updatedAt: now,
    revision: 1,
  })
  return { status: 'saved', id, revision: 1 }
}

async function changeNative(
  ctx: MutationCtx,
  operation: Exclude<NativeOperation, { kind: 'create' }>
): Promise<NativeResult> {
  promptRevision(operation.expectedRevision)
  const existing = await ctx.db.get(operation.id)
  if (!existing) return { status: 'deleted', id: operation.id, revision: 0 }
  const revision = promptRevision(existing.revision)
  if (revision !== operation.expectedRevision)
    return { status: 'conflict', id: operation.id, revision }
  if (operation.kind === 'remove') {
    await ctx.db.delete(operation.id)
    return { status: 'deleted', id: operation.id, revision: revision + 1 }
  }
  validatePromptFields(operation)
  await ctx.db.patch(operation.id, {
    ...buildPromptPatch(operation),
    updatedAt: Date.now(),
    revision: revision + 1,
  })
  return { status: 'saved', id: operation.id, revision: revision + 1 }
}

/** Native edits retry one immutable request. Receipts are historical, so the
 * caller reloads the list after success instead of treating it as current data. */
export const writeNative = mutation({
  args: { requestId: v.string(), operation: nativeOperation },
  handler: async (ctx, args): Promise<NativeResult> => {
    if (!/^[A-Za-z0-9._:-]{1,128}$/.test(args.requestId))
      throw new Error('Native prompt request ID is invalid')
    const digest = await operationDigest(args.operation)
    const previous = await ctx.db
      .query('terminalPromptWrites')
      .withIndex('by_request', q => q.eq('requestId', args.requestId))
      .unique()
    if (previous) {
      if (previous.digest !== digest)
        throw new Error('A native prompt request ID cannot be reused with different content')
      return previous.result
    }
    const result =
      args.operation.kind === 'create'
        ? await createNative(ctx, args.operation)
        : await changeNative(ctx, args.operation)
    await ctx.db.insert('terminalPromptWrites', {
      requestId: args.requestId,
      digest,
      result,
      createdAt: Date.now(),
    })
    return result
  },
})
