import {
  app,
  BrowserWindow,
  Menu,
  screen,
  session,
  type WebContents,
  type WebPreferences,
} from 'electron'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'
import { writeFile } from 'node:fs/promises'
import path from 'node:path'
import windowStateKeeper from 'electron-window-state'
import { initTelemetry, shutdownTelemetry, emitLog } from './telemetry'
import { configManager } from './config'
import { loadZoomLevel } from './zoom'
import { applicationMenuTemplate, bindWindowBehavior } from './menu'
import { registerAllHandlers } from './ipc'
import { MainWindowLifecycle } from './windowLifecycle'
import { manageWindowsWindowState } from './windowsWindowState'
import { getDispatcher } from './workers/dispatcher'
import { runOfflineSync } from './workers/offlineSync'
import { stopSharedClient } from './services/copilotClient'
import { initRalphService, shutdownRalphService } from './services/ralphService'
import { IPC_PUSH } from '../src/ipc/contracts'
import { validateUrl } from '../src/utils/networkSecurity'
import {
  resolveWindowBounds as resolveWindowBoundsPure,
  type DisplayInfo,
} from '../src/utils/windowGeometry'
import { startupTimer } from '../perf/startup-timing'
import {
  persistPackageSmokeResult,
  qualifyPackageDependencies,
  requirePackageRenderer,
  waitForMountedRenderer,
  type PackageSmokeResult,
} from './packageQualification'

// Initialize OpenTelemetry before anything else touches HTTP/DNS
await initTelemetry()

// Enable CDP remote debugging when BUDDY_DEBUG_PORT is set (e.g. via runApp.debug.ps1)
const debugPort = process.env.BUDDY_DEBUG_PORT
if (debugPort) {
  app.commandLine.appendSwitch('remote-debugging-port', debugPort)
  console.log(`[Debug] CDP remote debugging enabled on port ${debugPort}`)
}

const __dirname = path.dirname(fileURLToPath(import.meta.url))

// Ensure Windows taskbar always shows our icon (prevents fallback to generic Electron icon)
if (process.platform === 'win32') {
  app.setAppUserModelId('com.hemsoft.buddy')
}

// The built directory structure
//
// ├─┬─┬ dist
// │ │ └── index.html
// │ │
// │ ├─┬ dist-electron
// │ │ ├── main.js
// │ │ └── preload.mjs
// │
process.env.APP_ROOT = path.join(__dirname, '..')

export function parseUrlOrigin(value: string | undefined): string | null {
  if (!value) return null

  try {
    return new URL(value).origin
  } catch (_error: unknown) {
    return null
  }
}

const VITE_DEV_SERVER_URL = process.env['VITE_DEV_SERVER_URL']
const VITE_DEV_SERVER_ORIGIN = parseUrlOrigin(VITE_DEV_SERVER_URL)
const RENDERER_DIST = path.join(process.env.APP_ROOT, 'dist')

process.env.VITE_PUBLIC = VITE_DEV_SERVER_URL
  ? path.join(process.env.APP_ROOT, 'public')
  : RENDERER_DIST

const BROWSER_WEBVIEW_PARTITION = 'persist:browser'
const PACKAGE_SMOKE_OUTPUT = process.env.BUDDY_PACKAGE_SMOKE_FILE
const IS_ELECTRON_E2E = process.env.BUDDY_ELECTRON_E2E === '1'
const mainRequire = createRequire(import.meta.url)

async function packageSmokeResult(window: BrowserWindow): Promise<PackageSmokeResult> {
  const dependencies = qualifyPackageDependencies(
    process.platform,
    process.arch,
    process.resourcesPath,
    mainRequire,
    mainRequire.resolve
  )
  const rendererLoaded = await requirePackageRenderer(
    waitForMountedRenderer(
      () =>
        window.webContents.executeJavaScript(
          'window.__buddyPreloadReady === true && document.readyState === "complete" && document.getElementById("root")?.childElementCount > 0'
        ),
      () => new Promise(resolve => setTimeout(resolve, 250))
    ),
    window.webContents.executeJavaScript('window.__buddyPreloadReady === true')
  )
  return {
    ok: true,
    platform: process.platform,
    arch: process.arch,
    ...dependencies,
    rendererLoaded,
  }
}

function recordPackageSmoke(window: BrowserWindow, outputPath: string): Promise<void> {
  return persistPackageSmokeResult(
    () => packageSmokeResult(window),
    async result => {
      await writeFile(outputPath, JSON.stringify(result))
    },
    code => {
      app.exit(code)
    }
  )
}

type WebviewAttachParams = {
  src?: string
  partition?: string
}

type MutableWebviewPreferences = WebPreferences & {
  preload?: string
  preloadURL?: string
}

function isAllowedWebviewSource(src: string | undefined): boolean {
  if (!src) return false
  try {
    validateUrl(src)
    return true
  } catch (_: unknown) {
    return false
  }
}

function hardenWebviewPreferences(webPreferences: MutableWebviewPreferences): void {
  delete webPreferences.preload
  delete webPreferences.preloadURL

  Object.assign(webPreferences, {
    allowRunningInsecureContent: false,
    contextIsolation: true,
    nodeIntegration: false,
    nodeIntegrationInSubFrames: false,
    partition: BROWSER_WEBVIEW_PARTITION,
    plugins: false,
    sandbox: true,
    webSecurity: true,
  })
}

function registerWebviewSecurityGuards(): void {
  session
    .fromPartition(BROWSER_WEBVIEW_PARTITION)
    .setPermissionRequestHandler((_webContents, _permission, callback) => callback(false))

  app.on('web-contents-created', (_event, contents: WebContents) => {
    contents.setWindowOpenHandler(() => ({ action: 'deny' }))

    contents.on('will-attach-webview', (event, webPreferences, params) => {
      const attachParams = params as WebviewAttachParams

      if (
        attachParams.partition !== BROWSER_WEBVIEW_PARTITION ||
        !isAllowedWebviewSource(attachParams.src)
      ) {
        event.preventDefault()
        return
      }

      hardenWebviewPreferences(webPreferences as MutableWebviewPreferences)
    })
  })
}

function resolveWindowBounds(state: { x?: number; y?: number; width: number; height: number }): {
  x?: number
  y?: number
  width: number
  height: number
} {
  const savedDisplayId = configManager.getUiValue('displayId')
  const savedDisplayBounds = configManager.getUiValue('displayBounds')
  const allDisplays: DisplayInfo[] = screen.getAllDisplays().map(d => ({
    id: d.id,
    bounds: d.bounds,
    workArea: d.workArea,
  }))
  const primaryDisplay = screen.getPrimaryDisplay()

  return resolveWindowBoundsPure(state, {
    savedDisplayId,
    savedDisplayBounds,
    allDisplays,
    primaryWorkArea: primaryDisplay.workArea,
    getMatchingDisplay: bounds => {
      const d = screen.getDisplayMatching(bounds)
      return { id: d.id, bounds: d.bounds, workArea: d.workArea }
    },
  })
}

function createBrowserWindow(): BrowserWindow {
  // Load window state (position, size, etc.)
  const mainWindowState = windowStateKeeper({
    defaultWidth: 1400,
    defaultHeight: 900,
  })

  const {
    x: windowX,
    y: windowY,
    width: windowWidth,
    height: windowHeight,
  } = resolveWindowBounds(mainWindowState)

  const createdWindow = new BrowserWindow({
    x: windowX,
    y: windowY,
    width: windowWidth,
    height: windowHeight,
    minWidth: 800,
    minHeight: 600,
    frame: false,
    icon: path.join(
      process.env.VITE_PUBLIC || path.join(process.env.APP_ROOT!, 'public'),
      process.platform === 'win32' ? 'icon.ico' : 'icon.png'
    ),
    webPreferences: {
      preload: path.join(__dirname, 'preload.mjs'),
      contextIsolation: true,
      nodeIntegration: false,
      // Required for the Bookmarks in-app browser. See docs/WEBVIEW-SECURITY.md.
      webviewTag: true,
      // Use isolated partition to prevent zoom level bleeding to other Electron apps
      partition: 'persist:buddy',
      // Set initial zoom from saved config
      zoomFactor: loadZoomLevel(),
    },
    title: 'Buddy',
    backgroundColor: '#1e1e1e',
  })

  // Let window state manager track window state
  manageWindowsWindowState(
    createdWindow,
    app.getPath('userData'),
    { x: windowX, y: windowY, width: windowWidth, height: windowHeight },
    () => {
      mainWindowState.manage(createdWindow)
    }
  )

  // Track which display the window is on when it moves or the user stops resizing
  const saveCurrentDisplay = () => {
    const bounds = createdWindow.getBounds()
    const currentDisplay = screen.getDisplayMatching(bounds)
    configManager.setUiValue('displayId', currentDisplay.id)
    configManager.setUiValue('displayBounds', currentDisplay.bounds)
    configManager.setUiValue('displayWorkArea', currentDisplay.workArea)
  }

  // Save display on user-initiated moves/resizes (not immediately at startup
  // to avoid cementing a bad placement as the new source of truth)
  createdWindow.on('moved', saveCurrentDisplay)
  createdWindow.on('resize', saveCurrentDisplay)

  // The main window hosts the Buddy UI, not arbitrary web content. Embedded browser
  // tabs use separate <webview> contents and are intentionally unaffected by this guard.
  const mainWebContents = createdWindow.webContents
  const blockMainWindowNavigation = (event: Electron.Event, navigationUrl: string) => {
    const currentUrl = mainWebContents.getURL()
    const isTrustedInitialRedirect =
      currentUrl === '' &&
      VITE_DEV_SERVER_ORIGIN !== null &&
      parseUrlOrigin(navigationUrl) === VITE_DEV_SERVER_ORIGIN
    if (isTrustedInitialRedirect || navigationUrl === currentUrl) return

    event.preventDefault()
    emitLog('WARN', 'Blocked navigation that would replace the main app UI', {
      'navigation.origin': parseUrlOrigin(navigationUrl) ?? 'invalid',
    })
  }
  mainWebContents.on('will-navigate', blockMainWindowNavigation)
  mainWebContents.on('will-redirect', blockMainWindowNavigation)

  createdWindow.webContents.on('did-finish-load', () => {
    createdWindow.webContents.send(IPC_PUSH.MAIN_PROCESS_MESSAGE, new Date().toLocaleString())
    startupTimer.mark('content-loaded')
    startupTimer.report()

    if (PACKAGE_SMOKE_OUTPUT) void recordPackageSmoke(createdWindow, PACKAGE_SMOKE_OUTPUT)
  })

  if (VITE_DEV_SERVER_URL) {
    createdWindow.loadURL(VITE_DEV_SERVER_URL)
  } else {
    createdWindow.loadFile(path.join(RENDERER_DIST, 'index.html'))
  }

  return createdWindow
}

const windowLifecycle = new MainWindowLifecycle(createBrowserWindow, bindWindowBehavior)

// Quit when all windows are closed, except on macOS.
app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    app.quit()
  }
})

app.on('activate', () => {
  // On OS X it's common to re-create a window in the app when the
  // dock icon is clicked and there are no other windows open.
  windowLifecycle.activate()
})

app.whenReady().then(() => {
  startupTimer.mark('app-ready')

  // The frameless window hides the menu bar on Windows/Linux, but macOS would
  // otherwise install Electron's default application menu. Set an explicit
  // menu so no defaults appear on any platform: macOS keeps the app and Edit
  // roles for standard accelerators, everything else gets an empty menu. All
  // in-app menus live in the custom TitleBar component and shortcuts bind
  // through before-input-event (bindWindowBehavior).
  Menu.setApplicationMenu(Menu.buildFromTemplate(applicationMenuTemplate(process.platform)))

  registerWebviewSecurityGuards()

  // Initialize config manager and attempt migration from env vars
  configManager.migrateFromEnv()
  configManager.migrateWeatherLocation()

  // Register process-wide IPC exactly once. Window-scoped handlers resolve the
  // live sender/current window instead of capturing the first BrowserWindow.
  registerAllHandlers(() => windowLifecycle.currentWindow)

  windowLifecycle.openWindow()
  startupTimer.mark('window-created')

  // Qualification modes exercise the renderer and native runtime without
  // starting network-dependent background services.
  if (PACKAGE_SMOKE_OUTPUT || IS_ELECTRON_E2E) return

  // Recover orphaned ralph loops from a previous session
  initRalphService()

  emitLog('INFO', 'Application started', { 'app.version': app.getVersion() })

  // Process missed schedules from when the app was closed, then start polling
  runOfflineSync()
    .then(result => {
      if (result.runsCreated > 0) {
        console.log(`[Startup] Offline sync created ${result.runsCreated} catch-up run(s)`)
        emitLog('INFO', 'Offline sync completed', { 'sync.runs_created': result.runsCreated })
      }
    })
    .catch(err => {
      console.warn('[Startup] Offline sync failed (non-fatal):', err)
      emitLog('WARN', 'Offline sync failed', { 'error.message': String(err) })
    })
    .finally(() => {
      // Start the task dispatcher (polls Convex for pending runs)
      if (!isQuitting) {
        getDispatcher().start()
      }
    })
})

// Stop the dispatcher and shared Copilot client when the app is quitting
let isQuitting = false
app.on('before-quit', event => {
  if (isQuitting) return
  isQuitting = true
  event.preventDefault()

  try {
    getDispatcher().stop()
    stopSharedClient()
    shutdownRalphService()
  } catch (err: unknown) {
    console.error('[Main] Sync shutdown error:', err)
  }

  const timeout = new Promise<void>(resolve => setTimeout(resolve, 5_000))
  Promise.race([shutdownTelemetry(), timeout]).finally(() => {
    app.quit()
  })
})
