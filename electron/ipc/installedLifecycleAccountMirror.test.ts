import { execFileSync } from 'node:child_process'
import { resolve } from 'node:path'
import { afterEach, expect, it, vi } from 'vitest'

const stored = vi.hoisted(() => ({ initial: {} as Record<string, unknown> }))
vi.mock('electron', () => ({
  ipcMain: { handle: vi.fn() },
  dialog: {},
  shell: {},
  safeStorage: { isEncryptionAvailable: () => false },
}))
vi.mock('electron-store', async () => ({
  default: (await import('../__mocks__/configStore')).createConfigStoreMock(stored),
}))
// Unit control only: no filesystem locks, app process or credential adapter.
vi.mock('../configLock', () => ({
  withConfigLock: (_path: string, operation: () => unknown) => operation(),
}))
vi.mock('../services/pollenCredentials', () => ({
  PollenCredentials: class {
    constructor() {
      throw new Error('No vault access in this control')
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
        if value['mode']=='fixture': result=module.config_fixture()
        elif value['mode']=='baseline': result=module.electron_config_baseline(value['original'],value['actual'])
        else:
            module.require_subset(value['original'],value['actual'],'old or post-baseline config')
            result=True
        return dict(ok=True,result=result)
    except RuntimeError as error:
        return dict(ok=False,error=str(error))
print(json.dumps([control(value) for value in json.loads(sys.stdin.buffer.read().decode('utf-8'))]))
`
type Config = {
  github: {
    accounts: Array<{
      username: string
      org: string
      usageProvider?: 'copilot' | 'codex'
      futureAccount?: unknown
    }>
    usageProviderOverrides: Record<string, string>
    futureGithub: unknown
  }
  ui: { theme: string; futureUi: unknown }
  futureTop: unknown
}
type ContractCase = { mode: string; original?: unknown; actual?: unknown }
type ContractResult = { ok: boolean; error?: string; result?: unknown }

function contracts(cases: ContractCase[]): ContractResult[] {
  return JSON.parse(
    execFileSync(
      process.platform === 'win32' ? 'python' : 'python3',
      ['-c', probe, resolve('scripts/native-smoke/linux-installed-lifecycle.py')],
      { input: JSON.stringify(cases), encoding: 'utf8' }
    )
  )
}

afterEach(() => {
  vi.unstubAllGlobals()
})

async function connectMirror(input: Config) {
  stored.initial = structuredClone(input) as unknown as Record<string, unknown>
  vi.resetModules()
  const { ipcMain } = await import('electron')
  const { configManager } = await import('../config')
  const { registerConfigHandlers } = await import('./configHandlers')
  const handlers = new Map<string, (...args: unknown[]) => unknown>()
  vi.mocked(ipcMain.handle).mockImplementation((channel, handler) => {
    handlers.set(channel, handler as (...args: unknown[]) => unknown)
  })
  registerConfigHandlers()
  const invoke = vi.fn(async (channel: string, ...args: unknown[]) =>
    handlers.get(channel)!({}, ...args)
  )
  vi.stubGlobal('window', { ipcRenderer: { invoke } })
  const { mirrorConnectedGitHubAccounts } =
    await import('../../src/hooks/useUsageProviderOverrides')
  return { configManager, handlers, invoke, mirrorConnectedGitHubAccounts }
}

async function observeNormalization(input: Config) {
  const { configManager, handlers, invoke, mirrorConnectedGitHubAccounts } =
    await connectMirror(input)
  // Real renderer projection -> IPC account parser -> real replacement and
  // provider reconciliation. Only the storage/OS boundaries are mocked.
  await expect(mirrorConnectedGitHubAccounts(input.github.accounts)).resolves.toMatchObject({
    success: true,
  })
  const normalized = structuredClone(configManager.getConfig()) as unknown as Config
  expect(normalized.github.accounts).toEqual([
    { username: 'fixture-lifecycle', org: 'fixture-org', usageProvider: 'copilot' },
  ])
  expect(normalized.github.usageProviderOverrides).toEqual({})
  expect(invoke).toHaveBeenCalledWith('config:sync-github-accounts', normalized.github.accounts)
  // The IPC parser also drops unsupported fields when called directly.
  expect(handlers.get('config:sync-github-accounts')!({}, input.github.accounts)).toMatchObject({
    success: true,
  })
  expect(configManager.getConfig().github.accounts).toEqual(normalized.github.accounts)

  const anotherUnknown = structuredClone(input)
  Object.assign(anotherUnknown.github.accounts[0], { otherExtension: true })
  await mirrorConnectedGitHubAccounts(anotherUnknown.github.accounts)
  const afterExtraExtension = structuredClone(configManager.getConfig()) as unknown as Config
  return { normalized, anotherUnknown, afterExtraExtension }
}

function unexpectedLossCases(input: Config, normalized: Config): ContractCase[] {
  const cases: ContractCase[] = []
  for (const change of [
    (value: Config) => {
      value.github.accounts[0].username = 'wrong-user'
    },
    (value: Config) => {
      value.github.accounts[0].org = 'wrong-org'
    },
    (value: Config) => {
      value.github.accounts[0].usageProvider = 'codex'
    },
    (value: Config) => {
      value.ui.theme = 'light'
    },
    (value: Config) => {
      delete (value.ui as Partial<Config['ui']>).futureUi
    },
    (value: Config) => {
      delete (value.github as Partial<Config['github']>).futureGithub
    },
    (value: Config) => {
      delete (value as Partial<Config>).futureTop
    },
  ]) {
    const damaged = structuredClone(normalized)
    change(damaged)
    cases.push({ mode: 'baseline', original: input, actual: damaged })
    cases.push({ mode: 'strict', original: normalized, actual: damaged })
  }
  return cases
}

function assertBaselineContract(input: Config, normalized: Config, results: ContractResult[]) {
  const [oldExpectation, baseline, strictBaseline, ...losses] = results
  expect(oldExpectation).toMatchObject({
    ok: false,
    error: expect.stringContaining('futureAccount'),
  })
  expect(baseline).toMatchObject({
    ok: true,
    result: {
      config: normalized,
      nativeWorkStarted: false,
      observedRemovals: [
        { path: 'github.accounts[0].futureAccount', before: { preserve: true }, after: 'absent' },
      ],
      unqualifiedBoundary: expect.stringContaining('Unknown GitHub account extensions'),
    },
  })
  expect(input.github.accounts[0].futureAccount).toEqual({ preserve: true })
  expect(strictBaseline.ok).toBe(true)
  expect(losses).toHaveLength(16)
  for (const loss of losses) expect(loss.ok).toBe(false)
}

it('measures real connected account normalization and rejects new losses against that baseline', async () => {
  const input = contracts([{ mode: 'fixture' }])[0].result as Config
  const { normalized, anotherUnknown, afterExtraExtension } = await observeNormalization(input)
  // Batch portable Python controls to avoid repeated interpreter startup on
  // Windows while keeping the normal test budget.
  const results = contracts([
    { mode: 'strict', original: input, actual: normalized },
    { mode: 'baseline', original: input, actual: normalized },
    { mode: 'strict', original: normalized, actual: normalized },
    ...unexpectedLossCases(input, normalized),
    // A surviving extension becomes part of the baseline too: later removal
    // is never exempted by the native/rollback contract.
    { mode: 'strict', original: input, actual: normalized },
    { mode: 'baseline', original: anotherUnknown, actual: afterExtraExtension },
  ])
  assertBaselineContract(input, normalized, results)
})
