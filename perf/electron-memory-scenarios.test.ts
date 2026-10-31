import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Page } from 'puppeteer-core'
import type { ScenarioMetrics } from './electron-memory-model'
import type { LaunchedRuntime } from './electron-memory-runtime'
import { runBrowserLifecycle, runTerminalLifecycle } from './electron-memory-scenarios'

const { collectScenario } = vi.hoisted(() => ({ collectScenario: vi.fn() }))
vi.mock('./electron-memory-runtime', () => ({ collectScenario }))

describe('memory scenario handle ownership', () => {
  let handles: Array<{ dispose: ReturnType<typeof vi.fn>; focus: ReturnType<typeof vi.fn> }>
  let page: Page
  let runtime: LaunchedRuntime
  let automatic: 'available' | 'missing' | 'timeout'
  let textarea: boolean
  let inputFailure: Error | undefined
  const options = { settleMs: 0, navigationCycles: 3 }

  function handle() {
    const result = {
      dispose: vi.fn().mockResolvedValue(undefined),
      focus: vi.fn().mockResolvedValue(undefined),
    }
    handles.push(result)
    return result
  }

  beforeEach(() => {
    vi.useFakeTimers()
    handles = []
    automatic = 'available'
    textarea = true
    inputFailure = undefined
    let terminalAttempt = 0
    page = {
      evaluate: vi.fn().mockResolvedValue(false),
      waitForSelector: vi.fn(async (selector: string) => {
        if (selector === '.xterm' && terminalAttempt++ % 2 === 0 && automatic !== 'available') {
          if (automatic === 'timeout') throw new Error('automatic terminal timed out')
          return null
        }
        return handle()
      }),
      $: vi.fn(async () => (textarea ? handle() : null)),
      keyboard: {
        type: vi.fn(async () => {
          if (inputFailure) throw inputFailure
        }),
        press: vi.fn().mockResolvedValue(undefined),
      },
    } as unknown as Page
    runtime = { page } as LaunchedRuntime
    collectScenario.mockImplementation(async (name: string) => {
      // No automation handle may keep a detached DOM subtree alive at a memory checkpoint.
      expect(handles.every(item => item.dispose.mock.calls.length === 1)).toBe(true)
      const kind = name === 'terminal-open' ? 'spawned-child' : 'webview'
      return {
        processes: name.endsWith('-open') ? [{ pid: 42, kind }] : [],
      } as unknown as ScenarioMetrics
    })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it.each(['available', 'missing', 'timeout'] as const)(
    'releases terminal handles with automatic terminal %s',
    async availability => {
      automatic = availability
      const scenarios: Record<string, ScenarioMetrics> = {}
      const result = runTerminalLifecycle(page, runtime, options, scenarios)
      await vi.runAllTimersAsync()
      await result
      expect(Object.keys(scenarios)).toEqual([
        'terminal-first-cleanup',
        'terminal-baseline',
        'terminal-open',
        'terminal-cleanup',
      ])
      expect(handles.every(item => item.dispose.mock.calls.length === 1)).toBe(true)
    }
  )

  it('permits a missing terminal textarea without retaining selector handles', async () => {
    textarea = false
    const result = runTerminalLifecycle(page, runtime, options, {})
    await vi.runAllTimersAsync()
    await result
    expect(page.keyboard.type).not.toHaveBeenCalled()
  })

  it('releases the textarea when input fails', async () => {
    inputFailure = new Error('input failed')
    const result = runTerminalLifecycle(page, runtime, options, {})
    const rejected = expect(result).rejects.toThrow('input failed')
    await vi.runAllTimersAsync()
    await rejected
    expect(handles).toHaveLength(3)
    expect(handles.every(item => item.dispose.mock.calls.length === 1)).toBe(true)
  })

  it('releases browser handles before every memory checkpoint', async () => {
    const scenarios: Record<string, ScenarioMetrics> = {}
    const result = runBrowserLifecycle(page, runtime, options, scenarios)
    await vi.runAllTimersAsync()
    await result
    expect(handles).toHaveLength(4)
    expect(handles.every(item => item.dispose.mock.calls.length === 1)).toBe(true)
  })
})
