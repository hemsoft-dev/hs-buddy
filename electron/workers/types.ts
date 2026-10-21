/**
 * Shared types for the worker dispatch system.
 */

import type { WorkerResult } from '../../src/types/worker'

export type { WorkerResult }

/** Job config shape (mirrors Convex schema) */
export interface JobConfig {
  // exec-worker
  command?: string
  cwd?: string
  env?: Record<string, string>
  timeout?: number
  shell?: 'powershell' | 'bash' | 'cmd'
  // ai-worker
  prompt?: string
  model?: string
  // skill-worker
  skillName?: string
  action?: string
  params?: unknown
}

/** Worker interface — every worker type must implement this */
export interface Worker {
  execute(config: JobConfig, signal?: AbortSignal): Promise<WorkerResult>
}
