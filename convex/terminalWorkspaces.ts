import { query, mutation } from './_generated/server'
import { convexToJson, v } from 'convex/values'
import type { Doc } from './_generated/dataModel'

const nodeValidator = v.object({
  id: v.string(),
  name: v.string(),
  color: v.optional(v.string()),
  parentId: v.optional(v.string()),
  sortOrder: v.number(),
  layout: v.any(),
})

function currentRevision(value: number | undefined): number {
  const revision = value ?? 0
  if (!Number.isSafeInteger(revision) || revision < 0 || revision >= Number.MAX_SAFE_INTEGER) {
    throw new Error('Workspace revision is invalid or exhausted')
  }
  return revision
}

function isJsonPrimitive(value: unknown): boolean {
  return value === null || typeof value === 'boolean' || typeof value === 'string'
}

function canonicalJson(value: unknown, depth = 0): string {
  if (depth > 64) throw new Error('Native workspace nesting exceeds its limit')
  if (isJsonPrimitive(value)) {
    return JSON.stringify(value)
  }
  if (typeof value === 'number' && Number.isFinite(value)) return JSON.stringify(value)
  if (Array.isArray(value))
    return `[${value.map(item => canonicalJson(item, depth + 1)).join(',')}]`
  if (
    typeof value === 'object' &&
    value !== null &&
    Object.getPrototypeOf(value) === Object.prototype
  ) {
    const fields = Object.entries(value).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    return `{${fields.map(([key, item]) => `${JSON.stringify(key)}:${canonicalJson(item, depth + 1)}`).join(',')}}`
  }
  throw new Error('Native workspace layouts must contain JSON values')
}

function retainedNativeReceipt(
  previous: NonNullable<Doc<'terminalWorkspaces'>['nativeWrite']>,
  revision: number,
  expectedRevision: number,
  digest: string
) {
  if (previous.digest !== digest || previous.baseRevision !== expectedRevision) {
    throw new Error('A native workspace request ID cannot be reused with different content')
  }
  return {
    status: revision === previous.revision ? ('saved' as const) : ('superseded' as const),
    revision,
    savedRevision: previous.revision,
  }
}

async function payloadDigest(
  nodes: Doc<'terminalWorkspaces'>['nodes'],
  activeNodeId: string | undefined
): Promise<string> {
  const bytes = new TextEncoder().encode(
    canonicalJson(convexToJson({ nodes, activeNodeId: activeNodeId ?? null }))
  )
  if (bytes.byteLength > 1024 * 1024) throw new Error('Native workspace exceeds one MiB')
  const digest = await crypto.subtle.digest('SHA-256', bytes)
  return Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, '0')).join('')
}

type TerminalWorkspaceNode = {
  id: string
  parentId?: string
}

function shouldRemoveNode(node: TerminalWorkspaceNode, idsToRemove: Set<string>): boolean {
  if (!node.parentId) return false
  return idsToRemove.has(node.parentId) && !idsToRemove.has(node.id)
}

function collectNodeIdsToRemove(nodes: TerminalWorkspaceNode[], rootNodeId: string): Set<string> {
  const idsToRemove = new Set<string>([rootNodeId])
  let changed = true
  while (changed) {
    changed = false
    for (const node of nodes) {
      if (shouldRemoveNode(node, idsToRemove)) {
        idsToRemove.add(node.id)
        changed = true
      }
    }
  }
  return idsToRemove
}

function resolveActiveNodeAfterRemoval(
  activeNodeId: string | undefined,
  idsToRemove: Set<string>,
  remainingNodes: TerminalWorkspaceNode[]
): string | undefined {
  if (!activeNodeId) return activeNodeId
  if (!idsToRemove.has(activeNodeId)) return activeNodeId
  return remainingNodes[0]?.id
}

/** Get the terminal workspace (singleton). */
export const getWorkspace = query({
  args: {},
  handler: async ctx => {
    return await ctx.db
      .query('terminalWorkspaces')
      .withIndex('by_key', q => q.eq('key', 'default'))
      .unique()
  },
})

/** Save the full workspace (upsert). */
export const saveWorkspace = mutation({
  args: {
    nodes: v.array(nodeValidator),
    activeNodeId: v.optional(v.string()),
  },
  handler: async (ctx, args) => {
    const existing = await ctx.db
      .query('terminalWorkspaces')
      .withIndex('by_key', q => q.eq('key', 'default'))
      .unique()

    const now = Date.now()
    const revision = currentRevision(existing?.revision) + 1
    if (existing) {
      await ctx.db.patch(existing._id, {
        nodes: args.nodes,
        activeNodeId: args.activeNodeId,
        updatedAt: now,
        revision,
      })
    } else {
      await ctx.db.insert('terminalWorkspaces', {
        key: 'default',
        nodes: args.nodes,
        activeNodeId: args.activeNodeId,
        createdAt: now,
        updatedAt: now,
        revision,
      })
    }
  },
})

/** Native compare-and-swap saves retain an immutable receipt for safe same-request retries. */
export const saveWorkspaceNative = mutation({
  args: {
    nodes: v.array(nodeValidator),
    activeNodeId: v.optional(v.string()),
    expectedRevision: v.number(),
    requestId: v.string(),
  },
  handler: async (ctx, args) => {
    currentRevision(args.expectedRevision)
    if (!/^[A-Za-z0-9._:-]{1,128}$/.test(args.requestId))
      throw new Error('Native workspace request ID is invalid')
    const existing = await ctx.db
      .query('terminalWorkspaces')
      .withIndex('by_key', q => q.eq('key', 'default'))
      .unique()
    const revision = currentRevision(existing?.revision)
    const previous = existing?.nativeWrite
    if (previous?.requestId === args.requestId) {
      const digest = await payloadDigest(args.nodes, args.activeNodeId)
      return retainedNativeReceipt(previous, revision, args.expectedRevision, digest)
    }
    if (args.expectedRevision !== revision)
      return { status: 'conflict' as const, revision, savedRevision: null }
    const digest = await payloadDigest(args.nodes, args.activeNodeId)
    const next = revision + 1
    const nativeWrite = {
      requestId: args.requestId,
      digest,
      baseRevision: revision,
      revision: next,
    }
    const now = Date.now()
    if (existing) {
      await ctx.db.patch(existing._id, {
        nodes: args.nodes,
        activeNodeId: args.activeNodeId,
        revision: next,
        nativeWrite,
        updatedAt: now,
      })
    } else {
      await ctx.db.insert('terminalWorkspaces', {
        key: 'default',
        nodes: args.nodes,
        activeNodeId: args.activeNodeId,
        revision: next,
        nativeWrite,
        createdAt: now,
        updatedAt: now,
      })
    }
    return { status: 'saved' as const, revision: next, savedRevision: next }
  },
})

/** Update a single node within the workspace. */
export const updateNode = mutation({
  args: {
    nodeId: v.string(),
    patch: v.object({
      name: v.optional(v.string()),
      color: v.optional(v.string()),
      parentId: v.optional(v.string()),
      sortOrder: v.optional(v.number()),
      layout: v.optional(v.any()),
    }),
  },
  handler: async (ctx, args) => {
    const workspace = await ctx.db
      .query('terminalWorkspaces')
      .withIndex('by_key', q => q.eq('key', 'default'))
      .unique()
    if (!workspace) return

    const nodes = workspace.nodes.map(node =>
      node.id === args.nodeId ? { ...node, ...args.patch } : node
    )

    await ctx.db.patch(workspace._id, {
      nodes,
      updatedAt: Date.now(),
      revision: currentRevision(workspace.revision) + 1,
    })
  },
})

/** Remove a node from the workspace. */
export const removeNode = mutation({
  args: { nodeId: v.string() },
  handler: async (ctx, args) => {
    const workspace = await ctx.db
      .query('terminalWorkspaces')
      .withIndex('by_key', q => q.eq('key', 'default'))
      .unique()
    if (!workspace) return

    const idsToRemove = collectNodeIdsToRemove(workspace.nodes, args.nodeId)
    const nodes = workspace.nodes.filter(n => !idsToRemove.has(n.id))
    const activeNodeId = resolveActiveNodeAfterRemoval(workspace.activeNodeId, idsToRemove, nodes)

    await ctx.db.patch(workspace._id, {
      nodes,
      activeNodeId,
      updatedAt: Date.now(),
      revision: currentRevision(workspace.revision) + 1,
    })
  },
})

/** Set the active node. */
export const setActiveNode = mutation({
  args: { nodeId: v.optional(v.string()) },
  handler: async (ctx, args) => {
    const workspace = await ctx.db
      .query('terminalWorkspaces')
      .withIndex('by_key', q => q.eq('key', 'default'))
      .unique()
    if (!workspace) return

    await ctx.db.patch(workspace._id, {
      activeNodeId: args.nodeId,
      updatedAt: Date.now(),
      revision: currentRevision(workspace.revision) + 1,
    })
  },
})
