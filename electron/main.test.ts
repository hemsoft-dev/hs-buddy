/**
 * Tests for electron/main.ts startup behavior.
 *
 * main.ts is an imperative entrypoint with top-level await. Its testable logic
 * (window geometry, config, telemetry) is extracted to individually-tested modules.
 * This test validates the module structure and lifecycle registration through
 * mocking of Electron APIs.
 */
import { afterAll, describe, it, expect, vi } from 'vitest'

vi.stubEnv('VITE_DEV_SERVER_URL', 'https://localhost:5173/')
afterAll(() => vi.unstubAllEnvs())

// Track lifecycle callbacks registered with app.on / app.whenReady
const appOnCalls: [string, (...args: unknown[]) => unknown][] = []
let whenReadyCb: (() => void) | null = null
let browserSessionPermissionHandler:
  | ((webContents: unknown, permission: string, callback: (allowed: boolean) => void) => void)
  | null = null
const mainWebContentsListeners = new Map<string, (...args: unknown[]) => void>()
let closedWindowCallback: (() => void) | null = null

const mockWin = {
  webContents: {
    on: vi.fn((event: string, handler: (...args: unknown[]) => void) => {
      mainWebContentsListeners.set(event, handler)
    }),
    send: vi.fn(),
    getURL: vi.fn(() => 'file:///mock/dist/index.html'),
  },
  on: vi.fn(),
  once: vi.fn((event: string, callback: () => void) => {
    if (event === 'closed') closedWindowCallback = callback
  }),
  isDestroyed: vi.fn(() => false),
  loadURL: vi.fn(),
  loadFile: vi.fn(),
  getBounds: vi.fn(() => ({ x: 100, y: 100, width: 1400, height: 900 })),
}

vi.mock('electron', () => ({
  app: {
    on: vi.fn((event: string, handler: (...args: unknown[]) => unknown) => {
      appOnCalls.push([event, handler])
    }),
    whenReady: vi.fn(() => ({
      then: (cb: () => void) => {
        whenReadyCb = cb
        return { catch: vi.fn() }
      },
    })),
    quit: vi.fn(),
    getVersion: vi.fn(() => '1.0.0-test'),
    setAppUserModelId: vi.fn(),
    getPath: vi.fn(() => '/mock/userData'),
    commandLine: { appendSwitch: vi.fn() },
  },
  BrowserWindow: Object.assign(
    vi.fn(function () {
      return mockWin
    }),
    { getAllWindows: vi.fn(() => []) }
  ),
  Menu: { setApplicationMenu: vi.fn(), buildFromTemplate: vi.fn() },
  screen: {
    getAllDisplays: vi.fn(() => []),
    getPrimaryDisplay: vi.fn(() => ({ workArea: { x: 0, y: 0, width: 1920, height: 1040 } })),
    getDisplayMatching: vi.fn(() => ({ id: 1, bounds: {}, workArea: {} })),
  },
  session: {
    fromPartition: vi.fn(() => ({
      setPermissionRequestHandler: vi.fn(
        (
          handler: (
            webContents: unknown,
            permission: string,
            callback: (allowed: boolean) => void
          ) => void
        ) => {
          browserSessionPermissionHandler = handler
        }
      ),
    })),
  },
}))

vi.mock('node:url', () => ({
  fileURLToPath: vi.fn(() => '/mock/dist-electron/main.js'),
}))

vi.mock('electron-window-state', () => ({
  default: vi.fn(() => ({ x: 100, y: 100, width: 1400, height: 900, manage: vi.fn() })),
}))

vi.mock('./windowsWindowState', () => ({
  manageWindowsWindowState: vi.fn(
    (_window: unknown, _directory: string, _initial: unknown, manage: () => void) => manage()
  ),
}))

vi.mock('./telemetry', () => ({
  initTelemetry: vi.fn(async () => undefined),
  shutdownTelemetry: vi.fn(async () => undefined),
  emitLog: vi.fn(),
}))

vi.mock('./config', () => ({
  configManager: {
    migrateFromEnv: vi.fn(),
    migrateWeatherLocation: vi.fn(),
    getUiValue: vi.fn(() => undefined),
    setUiValue: vi.fn(),
  },
}))

vi.mock('./zoom', () => ({ loadZoomLevel: vi.fn(() => 1.0) }))
vi.mock('./menu', () => ({
  bindWindowBehavior: vi.fn(),
  applicationMenuTemplate: vi.fn((platform: NodeJS.Platform) =>
    platform === 'darwin' ? [{ role: 'appMenu' }, { role: 'editMenu' }] : []
  ),
}))
vi.mock('./ipc', () => ({ registerAllHandlers: vi.fn() }))
const mockDispatcher = { start: vi.fn(), stop: vi.fn() }
vi.mock('./workers/dispatcher', () => ({
  getDispatcher: vi.fn(() => mockDispatcher),
}))
vi.mock('./workers/offlineSync', () => ({
  runOfflineSync: vi.fn(async () => ({
    runsCreated: 0,
    schedulesProcessed: 0,
    skipped: 0,
    errors: [],
  })),
}))
vi.mock('./services/copilotClient', () => ({ stopSharedClient: vi.fn() }))
vi.mock('./services/ralphService', () => ({
  initRalphService: vi.fn(),
  shutdownRalphService: vi.fn(),
}))
vi.mock('../src/utils/windowGeometry', () => ({
  resolveWindowBounds: vi.fn((state: { width: number; height: number }) => ({
    width: state.width,
    height: state.height,
  })),
}))

describe('main process lifecycle', () => {
  it('parses URL origins without exposing credentials or throwing on malformed input', async () => {
    const { parseUrlOrigin } = await import('./main')

    expect(parseUrlOrigin('https://user:secret@example.com/path?token=secret')).toBe(
      'https://example.com'
    )
    expect(parseUrlOrigin('://not-a-url')).toBeNull()
    expect(parseUrlOrigin(undefined)).toBeNull()
  })

  it('registers expected lifecycle hooks when imported', async () => {
    // Importing the module triggers top-level code
    await import('./main')

    const registeredEvents = appOnCalls.map(([event]) => event)
    expect(registeredEvents).toContain('window-all-closed')
    expect(whenReadyCb).toBeTypeOf('function')
  })

  it('whenReady callback executes the boot sequence', async () => {
    await import('./main')
    const { registerAllHandlers } = await import('./ipc')
    const { configManager } = await import('./config')
    const { bindWindowBehavior } = await import('./menu')
    const { initRalphService } = await import('./services/ralphService')
    const { manageWindowsWindowState } = await import('./windowsWindowState')

    // Invoke the whenReady callback to exercise the boot path
    expect(whenReadyCb).not.toBeNull()
    whenReadyCb!()

    // Verify the boot sequence ran: config migration, window behavior, IPC, ralph
    expect(configManager.migrateFromEnv).toHaveBeenCalled()
    expect(configManager.migrateWeatherLocation).toHaveBeenCalled()
    expect(
      vi.mocked(configManager.migrateWeatherLocation).mock.invocationCallOrder[0]
    ).toBeLessThan(vi.mocked(registerAllHandlers).mock.invocationCallOrder[0])
    expect(bindWindowBehavior).toHaveBeenCalledWith(mockWin)
    expect(registerAllHandlers).toHaveBeenCalled()
    expect(initRalphService).toHaveBeenCalled()
    expect(manageWindowsWindowState).toHaveBeenLastCalledWith(
      mockWin,
      '/mock/userData',
      { x: undefined, y: undefined, width: 1400, height: 900 },
      expect.any(Function)
    )

    await vi.waitFor(() => {
      expect(mockDispatcher.start).toHaveBeenCalledOnce()
    })
  })

  it('sets an explicit application menu so macOS never installs defaults', async () => {
    await import('./main')
    const { Menu } = await import('electron')
    const { applicationMenuTemplate } = await import('./menu')
    const builtMenu = { items: [] }
    const buildFromTemplate = vi.mocked(Menu.buildFromTemplate)
    buildFromTemplate.mockReturnValue(builtMenu as unknown as Electron.Menu)

    try {
      whenReadyCb!()

      expect(buildFromTemplate).toHaveBeenCalledWith(applicationMenuTemplate(process.platform))
      expect(Menu.setApplicationMenu).toHaveBeenCalledWith(builtMenu)
    } finally {
      buildFromTemplate.mockRestore()
    }
  })

  it('rebinds window behavior without re-registering IPC across repeated activations', async () => {
    await import('./main')
    const { BrowserWindow } = await import('electron')
    const { registerAllHandlers } = await import('./ipc')
    const { bindWindowBehavior } = await import('./menu')
    const activateCb = appOnCalls.find(([event]) => event === 'activate')?.[1]

    expect(whenReadyCb).not.toBeNull()
    expect(activateCb).toBeDefined()
    whenReadyCb!()
    const getWindow = vi.mocked(registerAllHandlers).mock.calls.at(-1)?.[0]
    const createCount = vi.mocked(BrowserWindow).mock.calls.length
    const bindCount = vi.mocked(bindWindowBehavior).mock.calls.length
    const registrationCount = vi.mocked(registerAllHandlers).mock.calls.length
    const createRecreatedWindow = () => ({
      ...mockWin,
      webContents: { ...mockWin.webContents, on: vi.fn(), send: vi.fn() },
      on: vi.fn(),
      once: vi.fn((event: string, callback: () => void) => {
        if (event === 'closed') closedWindowCallback = callback
      }),
    })
    const useNextWindow = (window: ReturnType<typeof createRecreatedWindow>) => {
      vi.mocked(BrowserWindow).mockImplementationOnce(
        class {
          constructor() {
            return window
          }
        } as unknown as typeof BrowserWindow
      )
    }
    const recreatedWindow = createRecreatedWindow()

    expect(getWindow).toBeTypeOf('function')
    expect(getWindow!()).toBe(mockWin)
    useNextWindow(recreatedWindow)

    expect(closedWindowCallback).not.toBeNull()
    closedWindowCallback!()
    activateCb!()

    expect(BrowserWindow).toHaveBeenCalledTimes(createCount + 1)
    expect(bindWindowBehavior).toHaveBeenNthCalledWith(bindCount + 1, recreatedWindow)
    expect(registerAllHandlers).toHaveBeenCalledTimes(registrationCount)
    expect(getWindow!()).toBe(recreatedWindow)

    const nextRecreatedWindow = createRecreatedWindow()
    useNextWindow(nextRecreatedWindow)
    expect(closedWindowCallback).not.toBeNull()
    closedWindowCallback!()
    activateCb!()

    expect(BrowserWindow).toHaveBeenCalledTimes(createCount + 2)
    expect(bindWindowBehavior).toHaveBeenNthCalledWith(bindCount + 2, nextRecreatedWindow)
    expect(registerAllHandlers).toHaveBeenCalledTimes(registrationCount)
    expect(getWindow!()).toBe(nextRecreatedWindow)
  })

  it('blocks renderer navigation and redirects from replacing the main app UI', async () => {
    await import('./main')
    const { emitLog } = await import('./telemetry')

    expect(whenReadyCb).not.toBeNull()
    whenReadyCb!()

    for (const eventName of ['will-navigate', 'will-redirect']) {
      const navigationHandler = mainWebContentsListeners.get(eventName)
      expect(navigationHandler).toBeDefined()

      const event = { preventDefault: vi.fn() }
      navigationHandler!(event, 'https://example.com/unexpected?token=secret')

      expect(event.preventDefault).toHaveBeenCalledOnce()
      expect(emitLog).toHaveBeenLastCalledWith(
        'WARN',
        'Blocked navigation that would replace the main app UI',
        { 'navigation.origin': 'https://example.com' }
      )

      const reloadEvent = { preventDefault: vi.fn() }
      navigationHandler!(reloadEvent, mockWin.webContents.getURL())

      expect(reloadEvent.preventDefault).not.toHaveBeenCalled()
    }

    const initialRedirectHandler = mainWebContentsListeners.get('will-redirect')
    const initialRedirectEvent = { preventDefault: vi.fn() }
    mockWin.webContents.getURL.mockReturnValueOnce('')
    initialRedirectHandler!(initialRedirectEvent, 'https://localhost:5173/redirected')

    expect(initialRedirectEvent.preventDefault).not.toHaveBeenCalled()

    const crossOriginRedirectEvent = { preventDefault: vi.fn() }
    mockWin.webContents.getURL.mockReturnValueOnce('')
    initialRedirectHandler!(crossOriginRedirectEvent, 'https://example.com/redirected')

    expect(crossOriginRedirectEvent.preventDefault).toHaveBeenCalledOnce()
  })

  it('registers webview attach and popup guardrails', async () => {
    await import('./main')
    const { session } = await import('electron')

    expect(whenReadyCb).not.toBeNull()
    whenReadyCb!()
    expect(session.fromPartition).toHaveBeenCalledWith('persist:browser')

    const webContentsCreatedCb = appOnCalls.find(([e]) => e === 'web-contents-created')?.[1]
    expect(webContentsCreatedCb).toBeDefined()

    const webContentsListeners = new Map<string, (...args: unknown[]) => void>()
    const setWindowOpenHandler = vi.fn()
    webContentsCreatedCb!(
      {},
      {
        on: vi.fn((event: string, handler: (...args: unknown[]) => void) => {
          webContentsListeners.set(event, handler)
        }),
        setWindowOpenHandler,
      }
    )

    expect(setWindowOpenHandler).toHaveBeenCalled()
    expect(setWindowOpenHandler.mock.calls[0][0]({ url: 'https://example.com/popup' })).toEqual({
      action: 'deny',
    })

    const attachHandler = webContentsListeners.get('will-attach-webview')
    expect(attachHandler).toBeDefined()

    const event = { preventDefault: vi.fn() }
    const webPreferences = {
      allowRunningInsecureContent: true,
      contextIsolation: false,
      nodeIntegration: true,
      nodeIntegrationInSubFrames: true,
      partition: 'persist:untrusted',
      plugins: true,
      preload: 'file:///tmp/preload.js',
      preloadURL: 'file:///tmp/preload.js',
      sandbox: false,
      webSecurity: false,
    }

    attachHandler!(event, webPreferences, {
      partition: 'persist:browser',
      src: 'https://example.com',
    })

    expect(event.preventDefault).not.toHaveBeenCalled()
    expect(webPreferences).toMatchObject({
      allowRunningInsecureContent: false,
      contextIsolation: true,
      nodeIntegration: false,
      nodeIntegrationInSubFrames: false,
      partition: 'persist:browser',
      plugins: false,
      sandbox: true,
      webSecurity: true,
    })
    expect('preload' in webPreferences).toBe(false)
    expect('preloadURL' in webPreferences).toBe(false)
  })

  it('blocks webviews with unsafe source or partition', async () => {
    await import('./main')

    expect(whenReadyCb).not.toBeNull()
    whenReadyCb!()

    const webContentsCreatedCb = appOnCalls.find(([e]) => e === 'web-contents-created')?.[1]
    expect(webContentsCreatedCb).toBeDefined()

    const webContentsListeners = new Map<string, (...args: unknown[]) => void>()
    webContentsCreatedCb!(
      {},
      {
        on: vi.fn((event: string, handler: (...args: unknown[]) => void) => {
          webContentsListeners.set(event, handler)
        }),
        setWindowOpenHandler: vi.fn(),
      }
    )

    const attachHandler = webContentsListeners.get('will-attach-webview')
    expect(attachHandler).toBeDefined()

    const unsafeSourceEvent = { preventDefault: vi.fn() }
    attachHandler!(
      unsafeSourceEvent,
      {},
      {
        partition: 'persist:browser',
        src: 'file:///etc/passwd',
      }
    )
    expect(unsafeSourceEvent.preventDefault).toHaveBeenCalled()

    const unsafePartitionEvent = { preventDefault: vi.fn() }
    attachHandler!(
      unsafePartitionEvent,
      {},
      {
        partition: 'persist:other',
        src: 'https://example.com',
      }
    )
    expect(unsafePartitionEvent.preventDefault).toHaveBeenCalled()

    const missingPartitionEvent = { preventDefault: vi.fn() }
    attachHandler!(
      missingPartitionEvent,
      {},
      {
        src: 'https://example.com',
      }
    )
    expect(missingPartitionEvent.preventDefault).toHaveBeenCalled()

    const missingSrcEvent = { preventDefault: vi.fn() }
    attachHandler!(
      missingSrcEvent,
      {},
      {
        partition: 'persist:browser',
      }
    )
    expect(missingSrcEvent.preventDefault).toHaveBeenCalled()
  })

  it('denies browser webview permission prompts by default', async () => {
    await import('./main')

    expect(whenReadyCb).not.toBeNull()
    whenReadyCb!()
    expect(browserSessionPermissionHandler).toBeTypeOf('function')

    const callback = vi.fn()
    browserSessionPermissionHandler!({}, 'camera', callback)
    expect(callback).toHaveBeenCalledWith(false)
  })

  it('window-all-closed handler calls app.quit on non-darwin', async () => {
    await import('./main')

    const { app } = await import('electron')
    const originalPlatform = process.platform
    Object.defineProperty(process, 'platform', { value: 'win32' })

    const windowAllClosedCb = appOnCalls.find(([e]) => e === 'window-all-closed')?.[1]
    expect(windowAllClosedCb).toBeDefined()
    windowAllClosedCb!()
    expect(app.quit).toHaveBeenCalled()

    Object.defineProperty(process, 'platform', { value: originalPlatform })
  })

  it('before-quit stops services and blocks delayed offline sync dispatcher restarts', async () => {
    await import('./main')

    const { app } = await import('electron')
    const { stopSharedClient } = await import('./services/copilotClient')
    const { shutdownRalphService } = await import('./services/ralphService')
    const { shutdownTelemetry } = await import('./telemetry')
    const { runOfflineSync } = await import('./workers/offlineSync')
    const mockRunOfflineSync = vi.mocked(runOfflineSync)
    const syncResult = {
      runsCreated: 0,
      schedulesProcessed: 0,
      skipped: 0,
      errors: [],
    }
    let resolveSuccess!: (value: typeof syncResult) => void
    let rejectFailure!: (reason: Error) => void
    const delayedSuccess = new Promise<typeof syncResult>(resolve => {
      resolveSuccess = resolve
    })
    const delayedFailure = new Promise<typeof syncResult>((_, reject) => {
      rejectFailure = reject
    })
    mockRunOfflineSync
      .mockImplementationOnce(() => delayedSuccess)
      .mockImplementationOnce(() => delayedFailure)
    mockDispatcher.start.mockClear()

    const beforeQuitCb = appOnCalls.find(([e]) => e === 'before-quit')?.[1]
    expect(beforeQuitCb).toBeDefined()

    expect(whenReadyCb).not.toBeNull()
    whenReadyCb!()
    whenReadyCb!()

    // Simulate the event object with preventDefault
    const event = { preventDefault: vi.fn() }
    beforeQuitCb!(event)

    // Verify sync cleanup calls
    expect(event.preventDefault).toHaveBeenCalled()
    expect(mockDispatcher.stop).toHaveBeenCalled()
    expect(stopSharedClient).toHaveBeenCalled()
    expect(shutdownRalphService).toHaveBeenCalled()

    // Verify telemetry shutdown is invoked (races with timeout before calling app.quit)
    expect(shutdownTelemetry).toHaveBeenCalled()

    // After the race resolves, app.quit() should be called
    await vi.waitFor(() => {
      expect(app.quit).toHaveBeenCalled()
    })

    resolveSuccess(syncResult)
    rejectFailure(new Error('offline sync failed during shutdown'))
    await Promise.allSettled([delayedSuccess, delayedFailure])
    await Promise.resolve()

    expect(mockDispatcher.start).not.toHaveBeenCalled()
  })
})
