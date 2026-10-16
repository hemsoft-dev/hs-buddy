import { convexTest } from 'convex-test'
import { expect, test } from 'vitest'
import schema from '../schema'
import { api } from '../_generated/api'

const modules = import.meta.glob('../**/*.*s')

test('lost create response retries once, even after the prompt was deleted', async () => {
  const t = convexTest(schema, modules)
  const args = {
    requestId: 'create-lost-response',
    operation: { kind: 'create' as const, title: '  Daily  ', content: 'one\r\ntwo' },
  }
  const first = await t.mutation(api.terminalPrompts.writeNative, args)
  expect(first.status).toBe('saved')
  expect(await t.mutation(api.terminalPrompts.writeNative, args)).toEqual(first)
  const prompts = await t.query(api.terminalPrompts.list)
  expect(prompts).toHaveLength(1)
  expect(prompts[0]).toMatchObject({ title: 'Daily', content: 'one\ntwo', revision: 1 })
  await t.mutation(api.terminalPrompts.remove, { id: first.id })
  expect(await t.mutation(api.terminalPrompts.writeNative, args)).toEqual(first)
  expect(await t.query(api.terminalPrompts.list)).toHaveLength(0)
})

test('request ID cannot be repurposed after its response was lost', async () => {
  const t = convexTest(schema, modules)
  const operation = { kind: 'create' as const, title: 'Task', content: 'original' }
  await t.mutation(api.terminalPrompts.writeNative, { requestId: 'immutable', operation })
  await expect(
    t.mutation(api.terminalPrompts.writeNative, {
      requestId: 'immutable',
      operation: { ...operation, content: 'changed' },
    })
  ).rejects.toThrow('cannot be reused')
  expect((await t.query(api.terminalPrompts.list))[0].content).toBe('original')
})

test('native update cannot overwrite a later Electron edit and its conflict stays immutable', async () => {
  const t = convexTest(schema, modules)
  const id = await t.mutation(api.terminalPrompts.create, { title: 'Task', content: 'old' })
  await t.mutation(api.terminalPrompts.update, { id, content: 'newer Electron content' })
  const args = {
    requestId: 'stale-edit',
    operation: {
      kind: 'update' as const,
      id,
      expectedRevision: 0,
      title: 'Task',
      content: 'old native draft',
    },
  }
  const conflict = await t.mutation(api.terminalPrompts.writeNative, args)
  expect(conflict).toEqual({ status: 'conflict', id, revision: 1 })
  expect(await t.mutation(api.terminalPrompts.writeNative, args)).toEqual(conflict)
  expect((await t.query(api.terminalPrompts.list))[0].content).toBe('newer Electron content')
})

test('retired update retry never undoes a later accepted edit', async () => {
  const t = convexTest(schema, modules)
  const id = await t.mutation(api.terminalPrompts.create, { title: 'Task', content: 'old' })
  const args = {
    requestId: 'first-edit',
    operation: {
      kind: 'update' as const,
      id,
      expectedRevision: 0,
      title: 'Task',
      content: 'native',
    },
  }
  const receipt = await t.mutation(api.terminalPrompts.writeNative, args)
  expect(receipt).toEqual({ status: 'saved', id, revision: 1 })
  await t.mutation(api.terminalPrompts.update, { id, content: 'newer' })
  expect(await t.mutation(api.terminalPrompts.writeNative, args)).toEqual(receipt)
  expect((await t.query(api.terminalPrompts.list))[0].content).toBe('newer')
})

test('stale deletion conflicts, accepted deletion and missing-record update are idempotent', async () => {
  const t = convexTest(schema, modules)
  const id = await t.mutation(api.terminalPrompts.create, { title: 'Task', content: 'old' })
  await t.mutation(api.terminalPrompts.update, { id, content: 'new' })
  expect(
    await t.mutation(api.terminalPrompts.writeNative, {
      requestId: 'stale-delete',
      operation: {
        kind: 'remove',
        id,
        expectedRevision: 0,
      },
    })
  ).toEqual({ status: 'conflict', id, revision: 1 })
  const args = {
    requestId: 'delete',
    operation: { kind: 'remove' as const, id, expectedRevision: 1 },
  }
  const receipt = await t.mutation(api.terminalPrompts.writeNative, args)
  expect(receipt).toEqual({ status: 'deleted', id, revision: 2 })
  expect(await t.mutation(api.terminalPrompts.writeNative, args)).toEqual(receipt)
  expect(
    await t.mutation(api.terminalPrompts.writeNative, {
      requestId: 'missing-update',
      operation: {
        kind: 'update',
        id,
        expectedRevision: 2,
        title: 'Task',
        content: 'resurrect',
      },
    })
  ).toEqual({ status: 'deleted', id, revision: 0 })
  expect(await t.query(api.terminalPrompts.list)).toHaveLength(0)
})

test('mark-used does not conflict with a content edit, and a no-op legacy update retains revision', async () => {
  const t = convexTest(schema, modules)
  const id = await t.mutation(api.terminalPrompts.create, { title: 'Task', content: 'old' })
  await t.mutation(api.terminalPrompts.update, { id })
  await t.mutation(api.terminalPrompts.markUsed, { id })
  expect(
    await t.mutation(api.terminalPrompts.writeNative, {
      requestId: 'after-use',
      operation: {
        kind: 'update',
        id,
        expectedRevision: 0,
        title: ' Task ',
        content: 'new\r\nbody',
      },
    })
  ).toEqual({ status: 'saved', id, revision: 1 })
  expect((await t.query(api.terminalPrompts.list))[0]).toMatchObject({
    content: 'new\nbody',
    revision: 1,
  })
})

test('invalid request IDs, oversized text, blank fields and invalid revisions write nothing', async () => {
  const t = convexTest(schema, modules)
  const operation = { kind: 'create' as const, title: 'Task', content: 'body' }
  await expect(
    t.mutation(api.terminalPrompts.writeNative, { requestId: '', operation })
  ).rejects.toThrow('request ID')
  await expect(
    t.mutation(api.terminalPrompts.writeNative, {
      requestId: 'large',
      operation: { ...operation, content: 'x'.repeat(128 * 1024) },
    })
  ).rejects.toThrow('128 KiB')
  for (const fields of [
    { title: ' ', content: 'body' },
    { title: 'Task', content: ' \r\n' },
  ]) {
    await expect(
      t.mutation(api.terminalPrompts.writeNative, {
        requestId: `blank-${fields.title}`,
        operation: { kind: 'create', ...fields },
      })
    ).rejects.toThrow()
  }
  const id = await t.mutation(api.terminalPrompts.create, { title: 'Task', content: 'body' })
  for (const expectedRevision of [-1, 0.5, Number.MAX_SAFE_INTEGER]) {
    await expect(
      t.mutation(api.terminalPrompts.writeNative, {
        requestId: 'bad-revision',
        operation: {
          kind: 'remove',
          id,
          expectedRevision,
        },
      })
    ).rejects.toThrow('revision')
  }
  expect(await t.run(ctx => ctx.db.query('terminalPromptWrites').collect())).toHaveLength(0)
})

test('native nonfinite sort orders cannot collide in a JSON request digest', async () => {
  const t = convexTest(schema, modules)
  for (const sortOrder of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
    await expect(
      t.mutation(api.terminalPrompts.writeNative, {
        requestId: 'nonfinite-order',
        operation: { kind: 'create', title: 'Task', content: 'body', sortOrder },
      })
    ).rejects.toThrow('sort order must be finite')
  }
  expect(await t.query(api.terminalPrompts.list)).toHaveLength(0)
  expect(await t.run(ctx => ctx.db.query('terminalPromptWrites').collect())).toHaveLength(0)
})
