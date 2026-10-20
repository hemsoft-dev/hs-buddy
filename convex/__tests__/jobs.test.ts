import { convexTest } from 'convex-test'
import { describe, test, expect } from 'vitest'
import schema from '../schema'
import { api } from '../_generated/api'

const modules = import.meta.glob('../**/*.*s')

describe('jobs', () => {
  const baseJob = {
    name: 'test-job',
    workerType: 'exec' as const,
    config: { command: 'echo hello', cwd: '/tmp', timeout: 5000, shell: 'bash' as const },
  }

  test('explicit native environment survives legacy edits and can be cleared explicitly', async () => {
    const t = convexTest(schema, modules)
    const env = { BUDDY_FIXTURE_VALUE: '雪 exact' }
    const id = await t.mutation(api.jobs.create, { ...baseJob, config: { ...baseJob.config, env } })
    await t.mutation(api.jobs.update, { id, config: { command: 'legacy edit' } })
    expect((await t.query(api.jobs.get, { id }))?.config.env).toEqual(env)
    await t.mutation(api.jobs.update, {
      id,
      config: { command: 'native clears environment', env: {} },
    })
    expect((await t.query(api.jobs.get, { id }))?.config.env).toEqual({})
  })

  test('anonymous clients can list jobs', async () => {
    const t = convexTest(schema, modules)
    const jobs = await t.query(api.jobs.list)
    expect(jobs).toEqual([])
  })

  test('create inserts a new job', async () => {
    const t = convexTest(schema, modules)
    const id = await t.mutation(api.jobs.create, baseJob)
    expect(id).toBeTruthy()

    const jobs = await t.query(api.jobs.list)
    expect(jobs).toHaveLength(1)
    expect(jobs[0].name).toBe('test-job')
    expect(jobs[0].workerType).toBe('exec')
  })

  test('create stores optional description and inputParams', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.jobs.create, {
      ...baseJob,
      name: 'parameterized-job',
      description: 'A described job',
      inputParams: [
        {
          name: 'env',
          type: 'string' as const,
          required: true,
          description: 'Environment name',
        },
      ],
    })

    const job = await t.query(api.jobs.getByName, { name: 'parameterized-job' })
    expect(job?.description).toBe('A described job')
    expect(job?.inputParams).toHaveLength(1)
    expect(job?.inputParams?.[0].name).toBe('env')
  })

  test('create throws on duplicate job name', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.jobs.create, baseJob)

    await expect(t.mutation(api.jobs.create, baseJob)).rejects.toThrow(
      'Job with name "test-job" already exists'
    )
  })

  test('listByType returns jobs matching worker type', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.jobs.create, {
      name: 'exec-job',
      workerType: 'exec',
      config: { command: 'ls' },
    })
    await t.mutation(api.jobs.create, {
      name: 'ai-job',
      workerType: 'ai',
      config: { prompt: 'hello' },
    })

    const execJobs = await t.query(api.jobs.listByType, { workerType: 'exec' })
    expect(execJobs).toHaveLength(1)
    expect(execJobs[0].name).toBe('exec-job')

    const aiJobs = await t.query(api.jobs.listByType, { workerType: 'ai' })
    expect(aiJobs).toHaveLength(1)
    expect(aiJobs[0].name).toBe('ai-job')
  })

  test('get returns job by ID', async () => {
    const t = convexTest(schema, modules)
    const id = await t.mutation(api.jobs.create, baseJob)

    const job = await t.query(api.jobs.get, { id })
    expect(job?.name).toBe('test-job')
  })

  test('getByName returns job by name', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.jobs.create, baseJob)

    const job = await t.query(api.jobs.getByName, { name: 'test-job' })
    expect(job?.name).toBe('test-job')
  })

  test('getByName returns null for non-existent name', async () => {
    const t = convexTest(schema, modules)
    const job = await t.query(api.jobs.getByName, { name: 'nope' })
    expect(job).toBeNull()
  })

  test('update modifies job fields', async () => {
    const t = convexTest(schema, modules)
    const id = await t.mutation(api.jobs.create, baseJob)

    await t.mutation(api.jobs.update, {
      id,
      name: 'renamed-job',
      description: 'Updated',
    })

    const job = await t.query(api.jobs.get, { id })
    expect(job?.name).toBe('renamed-job')
    expect(job?.description).toBe('Updated')
  })

  test('update throws when renaming to a name that already exists', async () => {
    const t = convexTest(schema, modules)
    await t.mutation(api.jobs.create, baseJob)
    const id2 = await t.mutation(api.jobs.create, {
      name: 'other-job',
      workerType: 'exec',
      config: {},
    })

    await expect(t.mutation(api.jobs.update, { id: id2, name: 'test-job' })).rejects.toThrow(
      'Job with name "test-job" already exists'
    )
  })

  test('update throws when job does not exist', async () => {
    const t = convexTest(schema, modules)
    const id = await t.mutation(api.jobs.create, baseJob)
    await t.mutation(api.jobs.remove, { id })

    await expect(t.mutation(api.jobs.update, { id, name: 'nope' })).rejects.toThrow()
  })

  test('remove deletes a job', async () => {
    const t = convexTest(schema, modules)
    const id = await t.mutation(api.jobs.create, baseJob)

    await t.mutation(api.jobs.remove, { id })

    const jobs = await t.query(api.jobs.list)
    expect(jobs).toHaveLength(0)
  })

  test('remove throws when job does not exist', async () => {
    const t = convexTest(schema, modules)
    const id = await t.mutation(api.jobs.create, baseJob)
    await t.mutation(api.jobs.remove, { id })

    await expect(t.mutation(api.jobs.remove, { id })).rejects.toThrow()
  })

  test('remove cascades to associated schedules', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)

    // Create a schedule linked to this job
    await t.mutation(api.schedules.create, {
      jobId,
      name: 'daily-schedule',
      cron: '0 9 * * *',
      enabled: true,
      missedPolicy: 'skip',
    })

    await t.mutation(api.jobs.remove, { id: jobId })

    const scheduleList = await t.query(api.schedules.list)
    expect(scheduleList).toHaveLength(0)
  })
})
