import { mkdtempSync, readFileSync, writeFileSync, existsSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { randomUUID } from 'node:crypto'
import { afterEach, beforeEach, expect, test } from 'vitest'
import { runConfigCompanion } from './config-companion-protocol'

let profile: string
let runId: string
beforeEach(() => {
  profile = mkdtempSync(join(tmpdir(), 'buddy-companion-protocol-'))
  runId = randomUUID()
  writeFileSync(join(profile, '.buddy-settings-fixture'), 'buddy settings fixture v1')
  writeFileSync(join(profile, 'companion-request.json'), JSON.stringify({ runId }))
})
afterEach(() => rmSync(profile, { recursive: true, force: true }))

test('a native save after 25 seconds stays inside real writes until its controller stops', async () => {
  let clock = 100
  let accent = '#111111'
  const writes: { clock: number; slug: string; accent: string }[] = []
  const receipt = await runConfigCompanion({
    profile,
    electron: 'fixture',
    now: () => clock,
    readAccent: () => accent,
    write: slug => {
      if (writes.length === 0) expect(existsSync(join(profile, 'companion-ready.json'))).toBe(false)
      writes.push({ clock, slug, accent })
    },
    wait: async milliseconds => {
      clock += milliseconds
      if (clock >= 25_100) accent = '#8246af'
      if (clock >= 25_150)
        writeFileSync(join(profile, 'companion-stop.json'), JSON.stringify({ runId }))
    },
    timeoutMs: 30_000,
  })
  expect(receipt.observedNativeSaveAt).toBe(25_100)
  expect(receipt.completedAt).toBe(25_150)
  expect(receipt.writesBeforeNativeSave).toBeGreaterThan(1)
  expect(receipt.writesAfterNativeSave).toBeGreaterThan(0)
  expect(writes.at(-1)).toEqual({
    clock: 25_150,
    slug: 'fixture-concurrent-complete',
    accent: '#8246af',
  })
  expect(JSON.parse(readFileSync(join(profile, 'companion-complete.json'), 'utf8'))).toEqual(
    receipt
  )
})

test('another run cannot stop the current writer', async () => {
  let clock = 0
  let accent = '#111111'
  writeFileSync(join(profile, 'companion-stop.json'), JSON.stringify({ runId: randomUUID() }))
  const receipt = await runConfigCompanion({
    profile,
    electron: 'fixture',
    now: () => clock,
    readAccent: () => accent,
    write: () => {},
    wait: async milliseconds => {
      clock += milliseconds
      accent = '#8246af'
      if (clock >= 100)
        writeFileSync(join(profile, 'companion-stop.json'), JSON.stringify({ runId }))
    },
    timeoutMs: 200,
  })
  expect(receipt.completedAt).toBe(100)
  expect(receipt.runId).toBe(runId)
})

test('a missing controller signal fails at the deadline without a passed receipt', async () => {
  let clock = 0
  await expect(
    runConfigCompanion({
      profile,
      electron: 'fixture',
      now: () => clock,
      readAccent: () => '#111111',
      write: () => {},
      wait: async milliseconds => {
        clock += milliseconds
      },
      timeoutMs: 100,
    })
  ).rejects.toThrow('before its deadline')
  expect(clock).toBe(100)
  expect(existsSync(join(profile, 'companion-complete.json'))).toBe(false)
})

test('stopping without a native change fails without a passed receipt', async () => {
  writeFileSync(join(profile, 'companion-stop.json'), JSON.stringify({ runId }))
  await expect(
    runConfigCompanion({
      profile,
      electron: 'fixture',
      readAccent: () => '#111111',
      write: () => {},
    })
  ).rejects.toThrow('No native save')
  expect(existsSync(join(profile, 'companion-complete.json'))).toBe(false)
})

test('a value already present at startup cannot count as a native save', async () => {
  await expect(
    runConfigCompanion({
      profile,
      electron: 'fixture',
      readAccent: () => '#8246af',
      write: () => {},
    })
  ).rejects.toThrow('must change during the current run')
  expect(existsSync(join(profile, 'companion-ready.json'))).toBe(false)
})
