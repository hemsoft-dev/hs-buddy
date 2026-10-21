import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { spawn } from 'node:child_process'
import { EventEmitter } from 'node:events'
import { Readable } from 'node:stream'

vi.mock('node:child_process', () => ({
  spawn: vi.fn(),
}))

vi.mock('../../src/utils/shellUtils', () => ({
  resolveExecConfig: vi.fn(
    (command: string, opts: { shell?: string; timeout?: number }, defaultTimeout: number) => ({
      timeout: opts.timeout ?? defaultTimeout,
      shellCmd: opts.shell === 'bash' ? '/bin/bash' : 'powershell',
      shellArgs: ['-c'],
      finalCommand: command,
    })
  ),
  // eslint-disable-next-line complexity
  buildWorkerResult: vi.fn(params => {
    const stdout = (params.stdout || '').trim()
    const stderr = (params.stderr || '').trim()
    if (params.killed && params.aborted) {
      return {
        success: false,
        error: 'Aborted',
        output: stdout || undefined,
        duration: params.elapsedMs,
        exitCode: params.exitCode ?? -1,
      }
    }
    if (params.killed) {
      return {
        success: false,
        error: `Timeout after ${params.timeout}ms`,
        output: stdout || undefined,
        duration: params.elapsedMs,
        exitCode: params.exitCode ?? -1,
      }
    }
    const exitCode = params.exitCode ?? -1
    if (exitCode === 0) {
      return { success: true, output: stdout || undefined, duration: params.elapsedMs, exitCode: 0 }
    }
    return {
      success: false,
      error: stderr || `Exit code: ${exitCode}`,
      output: stdout || undefined,
      duration: params.elapsedMs,
      exitCode,
    }
  }),
}))

import { execWorker } from './execWorker'

function createMockProcess() {
  const proc = new EventEmitter() as EventEmitter & {
    stdout: Readable
    stderr: Readable
    kill: ReturnType<typeof vi.fn>
    killed: boolean
    pid: number | undefined
  }
  proc.stdout = new Readable({ read() {} })
  proc.stderr = new Readable({ read() {} })
  proc.kill = vi.fn(() => {
    proc.killed = true
    return true
  })
  proc.killed = false
  proc.pid = 1234
  return proc
}

describe('execWorker', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('applies only explicit overrides while retaining inherited environment without changing the parent', async () => {
    const proc = createMockProcess()
    vi.mocked(spawn).mockReturnValue(proc as never)
    const original = process.env.BUDDY_FIXTURE_VALUE
    const promise = execWorker.execute({
      command: 'fixture',
      env: { BUDDY_FIXTURE_VALUE: '雪 exact' },
    })
    const options = vi.mocked(spawn).mock.calls[0][2]
    expect(options?.env?.BUDDY_FIXTURE_VALUE).toBe('雪 exact')
    expect(options?.env?.PATH).toBe(process.env.PATH)
    expect(options?.env).not.toBe(process.env)
    expect(process.env.BUDDY_FIXTURE_VALUE).toBe(original)
    proc.emit('close', 0)
    await promise
  })

  it('returns error when no command is specified', async () => {
    const result = await execWorker.execute({})
    expect(result.success).toBe(false)
    expect(result.error).toBe('No command specified in job config')
  })

  it('executes a command and returns stdout on success', async () => {
    vi.useRealTimers()
    const proc = createMockProcess()
    vi.mocked(spawn).mockReturnValue(proc as never)

    const promise = execWorker.execute({ command: 'echo hello' })

    // Simulate stdout data and process exit
    await new Promise(r => setTimeout(r, 10))
    proc.stdout.push(Buffer.from('hello\n'))
    proc.stdout.push(null)
    proc.emit('close', 0)

    const result = await promise
    expect(result.success).toBe(true)
    expect(result.output).toBe('hello')
  })

  it('returns stderr on non-zero exit code', async () => {
    vi.useRealTimers()
    const proc = createMockProcess()
    vi.mocked(spawn).mockReturnValue(proc as never)

    const promise = execWorker.execute({ command: 'fail' })

    await new Promise(r => setTimeout(r, 10))
    proc.stderr.push(Buffer.from('error output'))
    proc.stderr.push(null)
    proc.emit('close', 1)

    const result = await promise
    expect(result.success).toBe(false)
    expect(result.error).toContain('error output')
  })

  it('returns error when process spawn fails', async () => {
    const proc = createMockProcess()
    proc.pid = undefined
    vi.mocked(spawn).mockReturnValue(proc as never)

    const promise = execWorker.execute({ command: 'nonexistent' })

    proc.emit('error', new Error('ENOENT'))
    expect(vi.getTimerCount()).toBe(0)

    vi.useRealTimers()
    const result = await promise
    expect(result.success).toBe(false)
    expect(result.error).toContain('Spawn error: ENOENT')
  })

  it('handles abort signal', async () => {
    const proc = createMockProcess()
    vi.mocked(spawn).mockReturnValue(proc as never)

    const controller = new AbortController()
    const promise = execWorker.execute({ command: 'long-running' }, controller.signal)

    // Abort the signal
    controller.abort()
    proc.emit('close', null)

    vi.useRealTimers()
    const result = await promise
    expect(proc.kill).toHaveBeenCalledWith('SIGTERM')
    expect(result.success).toBe(false)
  })

  it('spawns with correct shell args', async () => {
    const proc = createMockProcess()
    vi.mocked(spawn).mockReturnValue(proc as never)

    const promise = execWorker.execute({ command: 'test cmd', shell: 'bash' })
    proc.emit('close', 0)

    vi.useRealTimers()
    await promise

    expect(spawn).toHaveBeenCalledWith(
      '/bin/bash',
      ['-c', 'test cmd'],
      expect.objectContaining({ stdio: ['ignore', 'pipe', 'pipe'] })
    )
  })

  it('kills the process on timeout and returns timeout error', async () => {
    const proc = createMockProcess()
    vi.mocked(spawn).mockReturnValue(proc as never)

    const promise = execWorker.execute({ command: 'hang', timeout: 5000 })

    // Advance time past the timeout threshold
    vi.advanceTimersByTime(5000)

    // The timeout handler fires SIGTERM
    expect(proc.kill).toHaveBeenCalledWith('SIGTERM')

    // Simulate process exit after being killed
    proc.emit('close', null)

    vi.useRealTimers()
    const result = await promise
    expect(result.success).toBe(false)
    expect(result.error).toContain('Timeout')
  })

  it('force-kills with SIGKILL if process survives SIGTERM after 5s', async () => {
    const proc = createMockProcess()
    vi.mocked(spawn).mockReturnValue(proc as never)

    const promise = execWorker.execute({ command: 'unkillable', timeout: 3000 })

    // Advance past timeout — triggers SIGTERM
    vi.advanceTimersByTime(3000)
    expect(proc.kill).toHaveBeenCalledWith('SIGTERM')
    expect(proc.killed).toBe(true)

    // Advance 5 more seconds — triggers SIGKILL
    vi.advanceTimersByTime(5000)
    expect(proc.kill).toHaveBeenCalledWith('SIGKILL')

    // Finally the process exits
    proc.emit('close', null)

    vi.useRealTimers()
    const result = await promise
    expect(result.success).toBe(false)
  })

  it('cancels the force-kill when the process exits after SIGTERM', async () => {
    const proc = createMockProcess()
    vi.mocked(spawn).mockReturnValue(proc as never)

    const promise = execWorker.execute({ command: 'killable', timeout: 3000 })

    // Advance past timeout — triggers SIGTERM
    vi.advanceTimersByTime(3000)
    expect(proc.kill).toHaveBeenCalledWith('SIGTERM')

    proc.emit('close', null)
    expect(vi.getTimerCount()).toBe(0)

    // Advance 5 more seconds — SIGKILL should NOT be sent
    vi.advanceTimersByTime(5000)
    expect(proc.kill).not.toHaveBeenCalledWith('SIGKILL')

    vi.useRealTimers()
    const result = await promise
    expect(result.success).toBe(false)
  })

  it('force-kills when SIGTERM emits an error and the process remains alive', async () => {
    const proc = createMockProcess()
    vi.mocked(spawn).mockReturnValue(proc as never)

    const promise = execWorker.execute({ command: 'broken', timeout: 3000 })

    vi.advanceTimersByTime(3000)
    expect(proc.kill).toHaveBeenCalledWith('SIGTERM')

    proc.emit('error', new Error('kill EPERM'))
    expect(vi.getTimerCount()).toBe(1)

    vi.advanceTimersByTime(5000)
    expect(proc.kill).toHaveBeenCalledWith('SIGKILL')

    proc.emit('close', null)

    vi.useRealTimers()
    const result = await promise
    expect(result.error).toContain('Timeout')
  })

  it('caps stdout at MAX_OUTPUT_SIZE', async () => {
    vi.useRealTimers()
    const proc = createMockProcess()
    vi.mocked(spawn).mockReturnValue(proc as never)

    const promise = execWorker.execute({ command: 'verbose' })

    await new Promise(r => setTimeout(r, 10))
    // First chunk fills stdout beyond MAX_OUTPUT_SIZE (512_000)
    const bigChunk = Buffer.alloc(530_000, 'x')
    proc.stdout.push(bigChunk)
    // Second chunk should be discarded since stdout.length >= MAX_OUTPUT_SIZE
    proc.stdout.push(Buffer.alloc(100_000, 'y'))
    proc.stdout.push(null)
    proc.emit('close', 0)

    await promise
    const { buildWorkerResult } = await import('../../src/utils/shellUtils')
    const lastCall = vi.mocked(buildWorkerResult).mock.calls.at(-1)?.[0] as {
      stdout?: string
    }
    // First chunk (530KB) was accepted, but second chunk (100KB) was rejected
    expect(lastCall?.stdout?.length).toBe(530_000)
  })

  it('caps stderr at MAX_OUTPUT_SIZE', async () => {
    vi.useRealTimers()
    const proc = createMockProcess()
    vi.mocked(spawn).mockReturnValue(proc as never)

    const promise = execWorker.execute({ command: 'verbose-err' })

    await new Promise(r => setTimeout(r, 10))
    // First chunk fills stderr beyond MAX_OUTPUT_SIZE
    proc.stderr.push(Buffer.alloc(530_000, 'e'))
    // Second chunk should be discarded
    proc.stderr.push(Buffer.alloc(100_000, 'f'))
    proc.stderr.push(null)
    proc.emit('close', 1)

    await promise
    const { buildWorkerResult } = await import('../../src/utils/shellUtils')
    const lastCall = vi.mocked(buildWorkerResult).mock.calls.at(-1)?.[0] as {
      stderr?: string
    }
    // Only first 530KB chunk accepted, second rejected
    expect(lastCall?.stderr?.length).toBe(530_000)
  })
})
