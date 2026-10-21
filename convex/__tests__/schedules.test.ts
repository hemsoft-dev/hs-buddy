import { convexTest } from 'convex-test'
import { describe, test, expect } from 'vitest'
import schema from '../schema'
import { api } from '../_generated/api'

const modules = import.meta.glob('../**/*.*s')

const baseJob = {
  name: 'sched-job',
  workerType: 'exec' as const,
  config: { command: 'run' },
}

describe('schedules', () => {
  test('anonymous clients can list schedules', async () => {
    const t = convexTest(schema, modules)
    const schedules = await t.query(api.schedules.list)
    expect(schedules).toEqual([])
  })

  test('create inserts a new schedule', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)

    const id = await t.mutation(api.schedules.create, {
      jobId,
      name: 'daily',
      cron: '0 9 * * 1-5',
      enabled: true,
      missedPolicy: 'skip',
    })

    expect(id).toBeTruthy()

    const schedules = await t.query(api.schedules.list)
    expect(schedules).toHaveLength(1)
    expect(schedules[0].name).toBe('daily')
    expect(schedules[0].enabled).toBe(true)
  })

  test('create calculates nextRunAt when enabled', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)

    await t.mutation(api.schedules.create, {
      jobId,
      name: 'enabled-sched',
      cron: '0 9 * * *',
      enabled: true,
      missedPolicy: 'skip',
    })

    const schedules = await t.query(api.schedules.list)
    expect(schedules[0].nextRunAt).toBeGreaterThan(Date.now() - 1000)
  })

  test('create does not set nextRunAt when disabled', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)

    await t.mutation(api.schedules.create, {
      jobId,
      name: 'disabled-sched',
      cron: '0 9 * * *',
      enabled: false,
      missedPolicy: 'skip',
    })

    const schedules = await t.query(api.schedules.list)
    expect(schedules[0].nextRunAt).toBeUndefined()
  })

  test('create throws when job does not exist', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    await t.mutation(api.jobs.remove, { id: jobId })

    await expect(
      t.mutation(api.schedules.create, {
        jobId,
        name: 'orphan',
        cron: '* * * * *',
        enabled: true,
        missedPolicy: 'skip',
      })
    ).rejects.toThrow()
  })

  test('listEnabled returns only enabled schedules', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)

    await t.mutation(api.schedules.create, {
      jobId,
      name: 'enabled-s',
      cron: '* * * * *',
      enabled: true,
      missedPolicy: 'skip',
    })
    await t.mutation(api.schedules.create, {
      jobId,
      name: 'disabled-s',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })

    const enabled = await t.query(api.schedules.listEnabled)
    expect(enabled).toHaveLength(1)
    expect(enabled[0].name).toBe('enabled-s')
  })

  test('get returns schedule with job info', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const schedId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'get-test',
      cron: '* * * * *',
      enabled: true,
      missedPolicy: 'skip',
    })

    const sched = await t.query(api.schedules.get, { id: schedId })
    expect(sched?.name).toBe('get-test')
    expect(sched?.job?.name).toBe('sched-job')
  })

  test('get returns null for non-existent schedule', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const schedId = await t.mutation(api.schedules.create, {
      jobId,
      name: 'temp',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })
    await t.mutation(api.schedules.remove, { id: schedId })

    const sched = await t.query(api.schedules.get, { id: schedId })
    expect(sched).toBeNull()
  })

  test('update modifies schedule fields', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.schedules.create, {
      jobId,
      name: 'old-name',
      cron: '0 9 * * *',
      enabled: true,
      missedPolicy: 'skip',
    })

    await t.mutation(api.schedules.update, { id, name: 'new-name', enabled: false })

    const sched = await t.query(api.schedules.get, { id })
    expect(sched?.name).toBe('new-name')
    expect(sched?.enabled).toBe(false)
  })

  test('update throws when schedule does not exist', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.schedules.create, {
      jobId,
      name: 'to-remove',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })
    await t.mutation(api.schedules.remove, { id })

    await expect(t.mutation(api.schedules.update, { id, name: 'nope' })).rejects.toThrow()
  })

  test('remove deletes a schedule', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.schedules.create, {
      jobId,
      name: 'deletable',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })

    await t.mutation(api.schedules.remove, { id })

    const schedules = await t.query(api.schedules.list)
    expect(schedules).toHaveLength(0)
  })

  test('remove throws when schedule does not exist', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.schedules.create, {
      jobId,
      name: 'ghost',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })
    await t.mutation(api.schedules.remove, { id })

    await expect(t.mutation(api.schedules.remove, { id })).rejects.toThrow()
  })

  test('toggle enables a disabled schedule', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.schedules.create, {
      jobId,
      name: 'toggle-test',
      cron: '0 9 * * *',
      enabled: false,
      missedPolicy: 'skip',
    })

    const newEnabled = await t.mutation(api.schedules.toggle, { id })
    expect(newEnabled).toBe(true)

    const sched = await t.query(api.schedules.get, { id })
    expect(sched?.enabled).toBe(true)
    expect(sched?.nextRunAt).toBeGreaterThan(0)
  })

  test('toggle disables an enabled schedule', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.schedules.create, {
      jobId,
      name: 'toggle-off',
      cron: '0 9 * * *',
      enabled: true,
      missedPolicy: 'skip',
    })

    const newEnabled = await t.mutation(api.schedules.toggle, { id })
    expect(newEnabled).toBe(false)

    const sched = await t.query(api.schedules.get, { id })
    expect(sched?.enabled).toBe(false)
    expect(sched?.nextRunAt).toBeUndefined()
  })

  test('toggle throws when schedule does not exist', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.schedules.create, {
      jobId,
      name: 'gone',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })
    await t.mutation(api.schedules.remove, { id })

    await expect(t.mutation(api.schedules.toggle, { id })).rejects.toThrow()
  })

  test('advanceNextRun updates nextRunAt and optionally lastRunAt', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.schedules.create, {
      jobId,
      name: 'advance-test',
      cron: '0 9 * * *',
      enabled: true,
      missedPolicy: 'skip',
    })

    const futureMs = Date.now() + 3600000
    await t.mutation(api.schedules.advanceNextRun, {
      id,
      nextRunAt: futureMs,
      lastRunAt: Date.now(),
    })

    const sched = await t.query(api.schedules.get, { id })
    expect(sched?.nextRunAt).toBe(futureMs)
    expect(sched?.lastRunAt).toBeGreaterThan(0)
  })

  test('advanceNextRun throws when schedule does not exist', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.schedules.create, {
      jobId,
      name: 'no-sched',
      cron: '* * * * *',
      enabled: false,
      missedPolicy: 'skip',
    })
    await t.mutation(api.schedules.remove, { id })

    await expect(
      t.mutation(api.schedules.advanceNextRun, { id, nextRunAt: Date.now() + 1000 })
    ).rejects.toThrow()
  })
})

describe('schedule validation while paused', () => {
  const paused = {
    name: 'Paused',
    cron: '0 9 * * 1-5',
    enabled: false,
    missedPolicy: 'skip' as const,
  }

  test('create rejects an invalid cron or timezone even when paused', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    await expect(
      t.mutation(api.schedules.create, { ...paused, jobId, cron: 'not a cron' })
    ).rejects.toThrow()
    await expect(
      t.mutation(api.schedules.create, { ...paused, jobId, timezone: 'Nowhere/Invalid' })
    ).rejects.toThrow()
    expect(await t.query(api.schedules.list)).toEqual([])
  })

  test('update rejects an invalid cron or timezone even when paused', async () => {
    const t = convexTest(schema, modules)
    const jobId = await t.mutation(api.jobs.create, baseJob)
    const id = await t.mutation(api.schedules.create, { ...paused, jobId })
    await expect(t.mutation(api.schedules.update, { id, cron: '99 99 * * *' })).rejects.toThrow()
    await expect(
      t.mutation(api.schedules.update, { id, timezone: 'Nowhere/Invalid' })
    ).rejects.toThrow()
    await t.mutation(api.schedules.update, { id, name: 'Renamed' })
    const stored = await t.query(api.schedules.get, { id })
    expect(stored?.cron).toBe('0 9 * * 1-5')
    expect(stored?.name).toBe('Renamed')
  })
})
