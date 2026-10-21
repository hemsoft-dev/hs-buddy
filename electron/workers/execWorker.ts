/**
 * Exec Worker — Executes shell commands (PowerShell, Bash, cmd).
 *
 * Uses spawn() (not exec()) to handle large output without buffer overflow.
 * Supports configurable shell, working directory, and timeout.
 */

import { spawn } from 'node:child_process'
import type { Worker, WorkerResult, JobConfig } from './types'
import { resolveExecConfig, buildWorkerResult } from '../../src/utils/shellUtils'

const DEFAULT_TIMEOUT = 30_000 // 30 seconds
const MAX_OUTPUT_SIZE = 512_000 // 512KB per stream

function missingCommandResult(start: number): WorkerResult {
  return {
    success: false,
    error: 'No command specified in job config',
    duration: Date.now() - start,
  }
}

export const execWorker: Worker = {
  async execute(config: JobConfig, signal?: AbortSignal): Promise<WorkerResult> {
    const start = Date.now()

    if (!config.command) return missingCommandResult(start)

    const { timeout, shellCmd, shellArgs, finalCommand } = resolveExecConfig(
      config.command,
      { shell: config.shell, timeout: config.timeout },
      DEFAULT_TIMEOUT,
      process.platform
    )

    return new Promise<WorkerResult>(resolve => {
      let stdout = ''
      let stderr = ''
      let killed = false
      let childExited = false
      let forceKillTimer: ReturnType<typeof setTimeout> | undefined

      const child = spawn(shellCmd, [...shellArgs, finalCommand], {
        cwd: config.cwd || undefined,
        env: { ...process.env, ...config.env },
        stdio: ['ignore', 'pipe', 'pipe'],
        windowsHide: true,
      })

      // Timeout handling
      const timer = setTimeout(() => {
        killed = true
        // Force kill after 5s if still alive
        forceKillTimer = setTimeout(() => {
          if (!childExited) child.kill('SIGKILL')
        }, 5_000)
        child.kill('SIGTERM')
      }, timeout)

      const clearKillTimers = () => {
        clearTimeout(timer)
        if (forceKillTimer) clearTimeout(forceKillTimer)
      }

      // AbortSignal handling
      const onAbort = () => {
        killed = true
        child.kill('SIGTERM')
      }
      signal?.addEventListener('abort', onAbort, { once: true })

      // Collect stdout
      child.stdout.on('data', (chunk: Buffer) => {
        if (stdout.length < MAX_OUTPUT_SIZE) {
          stdout += chunk.toString('utf8')
        }
      })

      // Collect stderr
      child.stderr.on('data', (chunk: Buffer) => {
        if (stderr.length < MAX_OUTPUT_SIZE) {
          stderr += chunk.toString('utf8')
        }
      })

      // Handle errors (e.g. command not found)
      child.on('error', err => {
        if (child.pid !== undefined) return

        childExited = true
        clearKillTimers()
        signal?.removeEventListener('abort', onAbort)
        resolve({
          success: false,
          error: `Spawn error: ${err.message}`,
          duration: Date.now() - start,
        })
      })

      // Process exit
      child.on('close', exitCode => {
        childExited = true
        clearKillTimers()
        signal?.removeEventListener('abort', onAbort)
        resolve(
          buildWorkerResult({
            killed,
            aborted: !!signal?.aborted,
            exitCode,
            stdout,
            stderr,
            elapsedMs: Date.now() - start,
            timeout,
            maxOutputSize: MAX_OUTPUT_SIZE,
          })
        )
      })
    })
  },
}
