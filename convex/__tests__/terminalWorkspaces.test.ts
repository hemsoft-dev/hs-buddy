import { convexTest } from 'convex-test'
import { describe, expect, test } from 'vitest'
import schema from '../schema'
import { api } from '../_generated/api'

const modules = import.meta.glob('../**/*.*s')

const paneLayout = { type: 'pane', terminalId: 'terminal-1' }

const nativeNodes = () => [
  {
    id: 'native',
    name: 'Native',
    sortOrder: 0,
    layout: { type: 'pane', id: 'pane-one', cwd: '' },
  },
]

test('first save and same immutable request have one persisted effect', async () => {
  const t = convexTest(schema, modules)
  const args = {
    nodes: nativeNodes(),
    activeNodeId: 'native',
    expectedRevision: 0,
    requestId: 'request-one',
  }
  await expect(t.mutation(api.terminalWorkspaces.saveWorkspaceNative, args)).resolves.toEqual({
    status: 'saved',
    revision: 1,
    savedRevision: 1,
  })
  const before = await t.query(api.terminalWorkspaces.getWorkspace)
  await expect(
    t.mutation(api.terminalWorkspaces.saveWorkspaceNative, {
      ...args,
      nodes: [{ ...args.nodes[0], layout: { cwd: '', id: 'pane-one', type: 'pane' } }],
    })
  ).resolves.toEqual({ status: 'saved', revision: 1, savedRevision: 1 })
  expect(await t.query(api.terminalWorkspaces.getWorkspace)).toEqual(before)
  expect(await t.run(ctx => ctx.db.query('terminalWorkspaces').collect())).toHaveLength(1)
})

test('a stale native save preserves the newer Electron layout', async () => {
  const t = convexTest(schema, modules)
  await t.mutation(api.terminalWorkspaces.saveWorkspace, {
    nodes: workspaceNodes(),
    activeNodeId: 'child',
  })
  const before = await t.query(api.terminalWorkspaces.getWorkspace)
  await expect(
    t.mutation(api.terminalWorkspaces.saveWorkspaceNative, {
      nodes: nativeNodes(),
      expectedRevision: 0,
      requestId: 'stale',
    })
  ).resolves.toEqual({ status: 'conflict', revision: 1, savedRevision: null })
  expect(await t.query(api.terminalWorkspaces.getWorkspace)).toEqual(before)
})

test('reusing a request ID with different content or base revision cannot mutate data', async () => {
  const t = convexTest(schema, modules)
  const args = { nodes: nativeNodes(), expectedRevision: 0, requestId: 'immutable' }
  await t.mutation(api.terminalWorkspaces.saveWorkspaceNative, args)
  const before = await t.query(api.terminalWorkspaces.getWorkspace)
  await expect(
    t.mutation(api.terminalWorkspaces.saveWorkspaceNative, { ...args, nodes: [] })
  ).rejects.toThrow('cannot be reused')
  await expect(
    t.mutation(api.terminalWorkspaces.saveWorkspaceNative, { ...args, expectedRevision: 1 })
  ).rejects.toThrow('cannot be reused')
  expect(await t.query(api.terminalWorkspaces.getWorkspace)).toEqual(before)
})

test('a known native receipt remains superseded after later Electron node and selection writes', async () => {
  const t = convexTest(schema, modules)
  const args = {
    nodes: nativeNodes(),
    activeNodeId: 'native',
    expectedRevision: 0,
    requestId: 'receipt',
  }
  await t.mutation(api.terminalWorkspaces.saveWorkspaceNative, args)
  await t.mutation(api.terminalWorkspaces.updateNode, {
    nodeId: 'native',
    patch: { name: 'Electron edit' },
  })
  await t.mutation(api.terminalWorkspaces.setActiveNode, {})
  await t.mutation(api.terminalWorkspaces.removeNode, { nodeId: 'not-present' })
  const before = await t.query(api.terminalWorkspaces.getWorkspace)
  expect(before?.revision).toBe(4)
  await expect(t.mutation(api.terminalWorkspaces.saveWorkspaceNative, args)).resolves.toEqual({
    status: 'superseded',
    revision: 4,
    savedRevision: 1,
  })
  expect(await t.query(api.terminalWorkspaces.getWorkspace)).toEqual(before)
})

test('an older uncertain request cannot overwrite a later accepted native request', async () => {
  const t = convexTest(schema, modules)
  const first = { nodes: nativeNodes(), expectedRevision: 0, requestId: 'first' }
  await t.mutation(api.terminalWorkspaces.saveWorkspaceNative, first)
  await expect(
    t.mutation(api.terminalWorkspaces.saveWorkspaceNative, {
      nodes: [],
      expectedRevision: 1,
      requestId: 'second',
    })
  ).resolves.toEqual({ status: 'saved', revision: 2, savedRevision: 2 })
  await expect(t.mutation(api.terminalWorkspaces.saveWorkspaceNative, first)).resolves.toEqual({
    status: 'conflict',
    revision: 2,
    savedRevision: null,
  })
  expect((await t.query(api.terminalWorkspaces.getWorkspace))?.nodes).toEqual([])
})

test('legacy records start at revision zero and retain their creation metadata', async () => {
  const t = convexTest(schema, modules)
  await t.run(ctx =>
    ctx.db.insert('terminalWorkspaces', {
      key: 'default',
      nodes: workspaceNodes(),
      createdAt: 123,
      updatedAt: 456,
    })
  )
  await expect(
    t.mutation(api.terminalWorkspaces.saveWorkspaceNative, {
      nodes: nativeNodes(),
      expectedRevision: 0,
      requestId: 'legacy',
    })
  ).resolves.toEqual({ status: 'saved', revision: 1, savedRevision: 1 })
  expect((await t.query(api.terminalWorkspaces.getWorkspace))?.createdAt).toBe(123)
})

test.each([-1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER])(
  'invalid or exhausted revision %s leaves the singleton absent',
  async expectedRevision => {
    const t = convexTest(schema, modules)
    await expect(
      t.mutation(api.terminalWorkspaces.saveWorkspaceNative, {
        nodes: nativeNodes(),
        expectedRevision,
        requestId: 'invalid-revision',
      })
    ).rejects.toThrow('revision')
    expect(await t.query(api.terminalWorkspaces.getWorkspace)).toBeNull()
  }
)

test.each([
  { name: 'Int64', value: 1n, changed: 2n },
  { name: 'Int64 minimum', value: -(2n ** 63n), changed: 0n },
  { name: 'Int64 maximum', value: 2n ** 63n - 1n, changed: 0n },
  { name: 'Bytes', value: new Uint8Array([1, 2]).buffer, changed: new Uint8Array([1, 3]).buffer },
  { name: 'NaN', value: NaN, changed: null },
  { name: 'infinity', value: Infinity, changed: -Infinity },
  { name: 'negative zero', value: -0, changed: 0 },
])(
  'native saves preserve $name and bind replay to its exact Convex value',
  async ({ value, changed }) => {
    const t = convexTest(schema, modules)
    const original = nativeNodes()[0]
    const args = {
      nodes: [{ ...original, layout: { ...original.layout, future: value } }],
      expectedRevision: 0,
      requestId: 'preserved-value',
    }
    await expect(t.mutation(api.terminalWorkspaces.saveWorkspaceNative, args)).resolves.toEqual({
      status: 'saved',
      revision: 1,
      savedRevision: 1,
    })
    const before = await t.query(api.terminalWorkspaces.getWorkspace)
    expect(before?.nodes[0].layout.future).toEqual(value)
    await expect(
      t.mutation(api.terminalWorkspaces.saveWorkspaceNative, {
        ...args,
        nodes: [{ ...original, layout: { future: value, ...original.layout } }],
      })
    ).resolves.toEqual({ status: 'saved', revision: 1, savedRevision: 1 })
    await expect(
      t.mutation(api.terminalWorkspaces.saveWorkspaceNative, {
        ...args,
        nodes: [{ ...original, layout: { ...original.layout, future: changed } }],
      })
    ).rejects.toThrow('cannot be reused')
    expect(await t.query(api.terminalWorkspaces.getWorkspace)).toEqual(before)
  }
)

test('invalid request IDs and oversized or too-deep payloads are rejected', async () => {
  const t = convexTest(schema, modules)
  const args = { nodes: nativeNodes(), expectedRevision: 0, requestId: 'valid' }
  await expect(
    t.mutation(api.terminalWorkspaces.saveWorkspaceNative, { ...args, requestId: ' ' })
  ).rejects.toThrow('request ID')
  await expect(
    t.mutation(api.terminalWorkspaces.saveWorkspaceNative, {
      ...args,
      nodes: [{ ...args.nodes[0], layout: 'x'.repeat(1024 * 1024) }],
    })
  ).rejects.toThrow('one MiB')
  let deep: unknown = null
  for (let i = 0; i < 66; i++) deep = [deep]
  await expect(
    t.mutation(api.terminalWorkspaces.saveWorkspaceNative, {
      ...args,
      nodes: [{ ...args.nodes[0], layout: deep }],
    })
  ).rejects.toThrow('nesting')
  expect(await t.query(api.terminalWorkspaces.getWorkspace)).toBeNull()
})
function workspaceNodes() {
  return [
    { id: 'root', name: 'Root', sortOrder: 0, layout: paneLayout },
    {
      id: 'child',
      name: 'Child',
      color: '#00ff00',
      parentId: 'root',
      sortOrder: 1,
      layout: { type: 'pane', terminalId: 'terminal-2' },
    },
    {
      id: 'grandchild',
      name: 'Grandchild',
      parentId: 'child',
      sortOrder: 2,
      layout: { type: 'pane', terminalId: 'terminal-3' },
    },
    {
      id: 'sibling',
      name: 'Sibling',
      parentId: 'root',
      sortOrder: 3,
      layout: { type: 'pane', terminalId: 'terminal-4' },
    },
  ]
}

describe('terminalWorkspaces', () => {
  test('getWorkspace returns null before a workspace is saved', async () => {
    const t = convexTest(schema, modules)
    await expect(t.query(api.terminalWorkspaces.getWorkspace)).resolves.toBeNull()
  })

  test('saveWorkspace creates and replaces the singleton workspace', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.terminalWorkspaces.saveWorkspace, {
      nodes: workspaceNodes(),
      activeNodeId: 'child',
    })

    const created = await t.query(api.terminalWorkspaces.getWorkspace)
    expect(created?.key).toBe('default')
    expect(created?.nodes).toHaveLength(4)
    expect(created?.activeNodeId).toBe('child')

    await t.mutation(api.terminalWorkspaces.saveWorkspace, {
      nodes: [{ id: 'solo', name: 'Solo', sortOrder: 0, layout: paneLayout }],
      activeNodeId: 'solo',
    })

    const updated = await t.query(api.terminalWorkspaces.getWorkspace)
    expect(updated?.nodes).toEqual([{ id: 'solo', name: 'Solo', sortOrder: 0, layout: paneLayout }])
    expect(updated?.activeNodeId).toBe('solo')
    expect(updated?.updatedAt).toBeGreaterThanOrEqual(created?.updatedAt ?? 0)
  })
})

describe('terminalWorkspaces node updates', () => {
  test('updateNode leaves missing workspaces unchanged', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.terminalWorkspaces.updateNode, {
      nodeId: 'missing',
      patch: { name: 'Ignored' },
    })
    await expect(t.query(api.terminalWorkspaces.getWorkspace)).resolves.toBeNull()
  })

  test('updateNode patches one node', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.terminalWorkspaces.saveWorkspace, {
      nodes: workspaceNodes(),
      activeNodeId: 'root',
    })
    await t.mutation(api.terminalWorkspaces.updateNode, {
      nodeId: 'child',
      patch: {
        name: 'Renamed child',
        color: '#ff00ff',
        parentId: 'sibling',
        sortOrder: 9,
        layout: { type: 'pane', terminalId: 'updated' },
      },
    })

    const workspace = await t.query(api.terminalWorkspaces.getWorkspace)
    const child = workspace?.nodes.find(node => node.id === 'child')
    expect(child).toMatchObject({
      name: 'Renamed child',
      color: '#ff00ff',
      parentId: 'sibling',
      sortOrder: 9,
      layout: { type: 'pane', terminalId: 'updated' },
    })
    expect(workspace?.nodes.find(node => node.id === 'root')?.name).toBe('Root')
  })
})

describe('terminalWorkspaces node removal', () => {
  test('removeNode removes descendants and moves active node to the first remaining node', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.terminalWorkspaces.saveWorkspace, {
      nodes: workspaceNodes(),
      activeNodeId: 'grandchild',
    })

    await t.mutation(api.terminalWorkspaces.removeNode, { nodeId: 'child' })

    const workspace = await t.query(api.terminalWorkspaces.getWorkspace)
    expect(workspace?.nodes.map(node => node.id)).toEqual(['root', 'sibling'])
    expect(workspace?.activeNodeId).toBe('root')
  })

  test('removeNode preserves an active node outside the removed subtree', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.terminalWorkspaces.removeNode, { nodeId: 'missing' })
    await expect(t.query(api.terminalWorkspaces.getWorkspace)).resolves.toBeNull()

    await t.mutation(api.terminalWorkspaces.saveWorkspace, {
      nodes: workspaceNodes(),
      activeNodeId: 'sibling',
    })
    await t.mutation(api.terminalWorkspaces.removeNode, { nodeId: 'child' })

    const workspace = await t.query(api.terminalWorkspaces.getWorkspace)
    expect(workspace?.nodes.map(node => node.id)).toEqual(['root', 'sibling'])
    expect(workspace?.activeNodeId).toBe('sibling')
  })

  test('removeNode clears active node when every node is removed', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.terminalWorkspaces.saveWorkspace, {
      nodes: [{ id: 'root', name: 'Root', sortOrder: 0, layout: paneLayout }],
      activeNodeId: 'root',
    })

    await t.mutation(api.terminalWorkspaces.removeNode, { nodeId: 'root' })

    const workspace = await t.query(api.terminalWorkspaces.getWorkspace)
    expect(workspace?.nodes).toEqual([])
    expect(workspace?.activeNodeId).toBeUndefined()
  })
})

describe('terminalWorkspaces active node', () => {
  test('setActiveNode updates and clears the active node', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.terminalWorkspaces.setActiveNode, { nodeId: 'ignored' })
    await expect(t.query(api.terminalWorkspaces.getWorkspace)).resolves.toBeNull()

    await t.mutation(api.terminalWorkspaces.saveWorkspace, {
      nodes: workspaceNodes(),
      activeNodeId: undefined,
    })
    await t.mutation(api.terminalWorkspaces.setActiveNode, { nodeId: 'child' })
    expect((await t.query(api.terminalWorkspaces.getWorkspace))?.activeNodeId).toBe('child')

    await t.mutation(api.terminalWorkspaces.setActiveNode, { nodeId: undefined })
    expect((await t.query(api.terminalWorkspaces.getWorkspace))?.activeNodeId).toBeUndefined()
  })
})
