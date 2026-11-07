import { execFileSync } from 'node:child_process'
import { resolve } from 'node:path'
import { act, render } from '@testing-library/react'
import { Allotment } from 'allotment'
import { afterEach, expect, it, vi } from 'vitest'
import { useAppLayout } from '../hooks/useAppLayout'
import { computeAppMetrics, DEFAULT_ASSISTANT_PANE_SIZE } from '../appUtils'

const stored = vi.hoisted(() => ({ initial: {} as Record<string, unknown> }))
vi.mock('electron', () => ({
  ipcMain: { handle: vi.fn() },
  dialog: {},
  shell: {},
  safeStorage: { isEncryptionAvailable: () => false },
}))
vi.mock('electron-store', async () => ({
  default: (await import('../../electron/__mocks__/configStore')).createConfigStoreMock(stored),
}))
vi.mock('../../electron/configLock', () => ({
  withConfigLock: (_path: string, operation: () => unknown) => operation(),
}))
vi.mock('../../electron/services/pollenCredentials', () => ({
  PollenCredentials: class {
    constructor() {
      throw new Error('No vault in this unit control')
    }
  },
  pollenCredentialErrorMessage: () => 'Unavailable in synthetic control',
}))

const probe = `
import importlib.util,json,sys
spec=importlib.util.spec_from_file_location('lifecycle',sys.argv[1])
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
def control(value):
    try:
        if value['mode']=='fixture':
            result=dict(config=module.config_fixture(),script=module.LAYOUT.SCRIPT,assistantDefault=module.LAYOUT.ASSISTANT_DEFAULT)
        elif value['mode']=='baseline':
            result=module.electron_config_baseline(value['original'],value['actual'],layout=value.get('layout'),phase=value.get('phase'))
        elif value['mode']=='restore':
            module.LAYOUT.require_restored(value['layout'],value['after']);result=True
        else:
            module.require_subset(value['original'],value['actual']);result=True
        return dict(ok=True,result=result)
    except RuntimeError as error: return dict(ok=False,error=str(error))
print(json.dumps([control(value) for value in json.loads(sys.stdin.buffer.read().decode('utf-8'))]))
`
type Config = { ui: { paneSizes: number[]; assistantOpen: boolean }; futureTop: unknown }
type Sample = {
  config: Config
  observedAtMs: number
  dpr: number
  viewport: { width: number; height: number }
  outer: { width: number; height: number }
  body: DOMRect
  activity: DOMRect
  split: DOMRect
  panes: DOMRect[]
}
type Observation = { phase: string; samples: Sample[] }
type Case = {
  mode: string
  original?: Config
  actual?: Config
  layout?: Observation
  phase?: string
  after?: Observation
}
type Result = { ok: boolean; error?: string; result?: unknown }
function controls(cases: Case[]): Result[] {
  return JSON.parse(
    execFileSync(
      process.platform === 'win32' ? 'python' : 'python3',
      ['-c', probe, resolve('scripts/native-smoke/linux-installed-lifecycle.py')],
      { input: JSON.stringify(cases), encoding: 'utf8' }
    )
  )
}

function LayoutConsumer() {
  const layout = useAppLayout()
  if (!layout.loaded) return null
  const { defaultSizes } = computeAppMetrics([], [], {}, layout.assistantOpen, layout.paneSizes)
  return (
    <main className="app-body">
      <div className="activity-bar" />
      <Allotment defaultSizes={defaultSizes} onChange={layout.handlePaneChange}>
        <Allotment.Pane minSize={200}>Sidebar</Allotment.Pane>
        <Allotment.Pane minSize={400}>Content</Allotment.Pane>
      </Allotment>
    </main>
  )
}

function measuredDOM() {
  const observers: Array<{ callback: ResizeObserverCallback; target?: Element }> = []
  vi.stubGlobal(
    'ResizeObserver',
    class {
      entry: { callback: ResizeObserverCallback; target?: Element }
      constructor(callback: ResizeObserverCallback) {
        this.entry = { callback }
        observers.push(this.entry)
      }
      observe(target: Element) {
        this.entry.target = target
      }
      disconnect() {}
      unobserve() {}
    }
  )
  for (const [name, value] of Object.entries({
    innerWidth: 1200,
    innerHeight: 820,
    outerWidth: 1200,
    outerHeight: 820,
    devicePixelRatio: 1,
  })) {
    vi.stubGlobal(name, value)
  }
  // Unit geometry replaces only the DOM/OS boundary. Pane widths/offsets come
  // from the real Allotment layout algorithm's written element styles.
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (
    this: HTMLElement
  ) {
    if (this.classList.contains('app-body')) return new DOMRect(0, 30, 1200, 768)
    if (this.classList.contains('activity-bar')) return new DOMRect(0, 30, 48, 768)
    if (this.classList.contains('split-view-view'))
      return new DOMRect(
        48 + Number.parseFloat(this.style.left || '0'),
        30,
        Number.parseFloat(this.style.width || '0'),
        768
      )
    return new DOMRect(48, 30, 1152, 768)
  })
  return observers
}

async function connectLayout(input: Config) {
  stored.initial = structuredClone(input) as unknown as Record<string, unknown>
  const { ipcMain } = await import('electron')
  const { registerConfigHandlers } = await import('../../electron/ipc/configHandlers')
  const { configManager } = await import('../../electron/config')
  const handlers = new Map<string, (...args: unknown[]) => unknown>()
  vi.mocked(ipcMain.handle).mockImplementation((channel, handler) => {
    handlers.set(channel, handler as (...args: unknown[]) => unknown)
  })
  registerConfigHandlers()
  const invoke = vi.fn(async (channel: string, ...args: unknown[]) =>
    handlers.get(channel)!({}, ...args)
  )
  Object.defineProperty(window, 'ipcRenderer', {
    configurable: true,
    value: { invoke, on: vi.fn(), off: vi.fn() },
  })
  return { configManager, invoke }
}

async function observeReflow(input: Config, script: string) {
  const { configManager, invoke } = await connectLayout(input)
  const observers = measuredDOM()
  vi.useFakeTimers()
  await act(async () => {
    render(<LayoutConsumer />)
  })
  expect(observers).toHaveLength(1)
  await act(async () => {
    observers[0].callback(
      [
        {
          contentBoxSize: [{ inlineSize: 1152, blockSize: 768 }],
        } as unknown as ResizeObserverEntry,
      ],
      {} as ResizeObserver
    )
  })
  expect(invoke).not.toHaveBeenCalledWith('config:set-pane-sizes', expect.anything())
  await act(async () => {
    await vi.advanceTimersByTimeAsync(400)
  })
  const actual = structuredClone(configManager.getConfig()) as unknown as Config
  expect(actual.ui.paneSizes).toHaveLength(3)
  expect(actual.ui.paneSizes[2]).toBe(DEFAULT_ASSISTANT_PANE_SIZE)
  expect(invoke).toHaveBeenCalledWith('config:set-pane-sizes', actual.ui.paneSizes)
  const sample: Sample = await window.eval(script)
  expect(sample.panes.map(pane => pane.width)).toEqual(actual.ui.paneSizes.slice(0, 2))
  const layout = {
    phase: 'electron-seed',
    samples: [
      { ...structuredClone(sample), observedAtMs: 0 },
      { ...structuredClone(sample), observedAtMs: 400 },
    ],
  }
  return { actual, layout }
}

function invalidObservations(layout: Observation): Observation[] {
  return [
    (value: Observation) => {
      value.samples = []
    },
    (value: Observation) => {
      value.samples[1].observedAtMs = 200
    },
    (value: Observation) => {
      value.samples[1].panes[0].width += 1
    },
    (value: Observation) => {
      value.samples.forEach(sample => {
        sample.outer.width = 1100
      })
    },
    (value: Observation) => {
      value.samples.forEach(sample => {
        sample.panes = sample.panes.slice(0, 1)
      })
    },
    (value: Observation) => {
      value.samples.forEach(sample => {
        sample.panes[0].x += 10
      })
    },
    (value: Observation) => {
      value.samples.forEach(sample => {
        sample.split.width -= 10
      })
    },
    (value: Observation) => {
      value.samples.forEach(sample => {
        sample.dpr = 0
      })
    },
  ].map(change => {
    const value = structuredClone(layout)
    change(value)
    return value
  })
}

function negativeCases(input: Config, actual: Config, layout: Observation): Case[] {
  const cases: Case[] = invalidObservations(layout).map(value => ({
    mode: 'baseline',
    original: input,
    actual,
    layout: value,
    phase: 'electron-seed',
  }))
  for (const panes of [
    [0, 1152, 350],
    [307, 845],
    [307, 845, 0],
    [Number.NaN, 845, 350],
    [307, 845, 350, 4],
    [400, 752, 350],
  ]) {
    const damaged = structuredClone(actual)
    damaged.ui.paneSizes = panes
    cases.push({
      mode: 'baseline',
      original: input,
      actual: damaged,
      layout,
      phase: 'electron-seed',
    })
  }
  for (const phase of ['native-upgrade', 'electron-installed-rollback']) {
    cases.push({ mode: 'baseline', original: input, actual, layout: { ...layout, phase }, phase })
  }
  const lostUnknown = structuredClone(actual)
  delete (lostUnknown as Partial<Config>).futureTop
  cases.push({
    mode: 'baseline',
    original: input,
    actual: lostUnknown,
    layout,
    phase: 'electron-seed',
  })
  const changedLater = structuredClone(actual)
  changedLater.ui.paneSizes[0] += 1
  cases.push({ mode: 'strict', original: actual, actual: changedLater })
  cases.push({ mode: 'strict', original: actual, actual: lostUnknown })
  return cases
}

function internallyConsistentBadLayouts(
  input: Config,
  actual: Config,
  layout: Observation
): Case[] {
  return [
    [400, 752, 350],
    [...actual.ui.paneSizes.slice(0, 2), 399],
  ].map(panes => {
    const damaged = structuredClone(actual)
    damaged.ui.paneSizes = panes
    const observation = structuredClone(layout)
    for (const sample of observation.samples) {
      sample.config = structuredClone(damaged)
      sample.panes[0].width = panes[0]
      sample.panes[1].width = panes[1]
      sample.panes[1].x = sample.split.x + panes[0]
    }
    return {
      mode: 'baseline',
      original: input,
      actual: damaged,
      layout: observation,
      phase: 'electron-seed',
    }
  })
}

function restorationCases(layout: Observation): Case[] {
  const before = { ...structuredClone(layout), phase: 'electron-restart' }
  const after = { ...structuredClone(layout), phase: 'electron-installed-rollback' }
  const changed = structuredClone(after)
  changed.samples[1].panes[0].width += 10
  return [
    { mode: 'restore', layout: before, after },
    { mode: 'restore', layout: before, after: changed },
  ]
}

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

it('measures Allotment startup reflow through the real hook and IPC without excusing later data loss', async () => {
  const fixture = controls([{ mode: 'fixture' }])[0].result as {
    config: Config
    script: string
    assistantDefault: number
  }
  expect(fixture.assistantDefault).toBe(DEFAULT_ASSISTANT_PANE_SIZE)
  const { actual, layout } = await observeReflow(fixture.config, fixture.script)
  const [restored, changedGeometry] = restorationCases(layout)
  const results = controls([
    { mode: 'baseline', original: fixture.config, actual }, // Old account-only contract, no measured layout.
    { mode: 'baseline', original: fixture.config, actual, layout, phase: 'electron-seed' },
    { mode: 'strict', original: actual, actual },
    restored,
    ...negativeCases(fixture.config, actual, layout),
    ...internallyConsistentBadLayouts(fixture.config, actual, layout),
    changedGeometry,
  ])
  expect(results[0]).toMatchObject({ ok: false })
  expect(results[1]).toMatchObject({
    ok: true,
    result: {
      config: actual,
      layoutNormalization: [
        {
          path: 'ui.paneSizes',
          before: [320, 880],
          after: actual.ui.paneSizes,
          phase: 'electron-seed',
        },
      ],
    },
  })
  expect(results[2].ok).toBe(true)
  expect(results[3].ok).toBe(true)
  expect(results.slice(4)).toHaveLength(22)
  for (const rejected of results.slice(4)) expect(rejected.ok).toBe(false)
  expect(fixture.config.ui.paneSizes).toEqual([320, 880])
})
