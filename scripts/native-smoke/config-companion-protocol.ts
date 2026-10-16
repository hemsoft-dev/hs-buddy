import { readFileSync, writeFileSync, existsSync, renameSync } from 'node:fs'
import { join } from 'node:path'

type CompanionOptions = {
  profile: string
  write: (slug: string) => void
  readAccent: () => string
  electron: string
  now?: () => number
  wait?: (milliseconds: number) => Promise<void>
  timeoutMs?: number
}

function writeSignal<T extends { runId: string }>(profile: string, name: string, data: T) {
  const target = join(profile, `companion-${name}.json`)
  const temporary = `${target}.${data.runId}.tmp`
  writeFileSync(temporary, JSON.stringify(data))
  renameSync(temporary, target)
}

function readRequest(profile: string): string {
  const request: unknown = JSON.parse(readFileSync(join(profile, 'companion-request.json'), 'utf8'))
  if (
    typeof request !== 'object' ||
    request === null ||
    !('runId' in request) ||
    typeof request.runId !== 'string' ||
    !/^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/.test(request.runId)
  )
    throw new Error('Invalid companion request')
  return request.runId
}

function stopRequested(profile: string, runId: string): boolean {
  const stop = join(profile, 'companion-stop.json')
  return existsSync(stop) && JSON.parse(readFileSync(stop, 'utf8')).runId === runId
}

function companionClock(options: CompanionOptions) {
  return {
    now: options.now ?? Date.now,
    wait:
      options.wait ??
      (milliseconds => new Promise<void>(resolve => setTimeout(resolve, milliseconds))),
    timeout: options.timeoutMs ?? 120_000,
  }
}

/** Coordinate a real config writer with its owned native-input controller. */
export async function runConfigCompanion(options: CompanionOptions) {
  const { profile, write, readAccent, electron } = options
  if (
    readFileSync(join(profile, '.buddy-settings-fixture'), 'utf8') !== 'buddy settings fixture v1'
  )
    throw new Error('Missing fixture marker')
  const runId = readRequest(profile)
  const expectedAccent = '#8246af'
  if (readAccent() === expectedAccent)
    throw new Error('Native accent must change during the current run')
  const { now, wait, timeout } = companionClock(options)
  const startedAt = now()
  let writes = 0
  let observedNativeSaveAt: number | undefined
  let writesBeforeNativeSave: number | undefined
  // Readiness means the real writer has already run. The controller may start
  // native input immediately after observing this signal.
  write(`fixture-concurrent-${writes++}`)
  writeSignal(profile, 'ready', { ready: true, runId, electron, startedAt })
  while (now() - startedAt < timeout) {
    write(`fixture-concurrent-${writes++}`)
    if (observedNativeSaveAt === undefined && readAccent() === expectedAccent) {
      observedNativeSaveAt = now()
      writesBeforeNativeSave = writes
    }
    if (stopRequested(profile, runId)) {
      if (observedNativeSaveAt === undefined || writesBeforeNativeSave === undefined)
        throw new Error('No native save was observed during current-run writes')
      // This final real config write must retain the native edit, and happens
      // after that edit was read back while the companion was still active.
      write('fixture-concurrent-complete')
      writes++
      const result = {
        result: 'passed',
        runId,
        writes,
        electron,
        credentialAccess: false,
        startedAt,
        observedNativeSaveAt,
        writesBeforeNativeSave,
        writesAfterNativeSave: writes - writesBeforeNativeSave,
        completedAt: now(),
      }
      writeSignal(profile, 'complete', result)
      return result
    }
    await wait(25)
  }
  throw new Error('Native-input controller did not stop the config companion before its deadline')
}
