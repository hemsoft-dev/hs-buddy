import { readFileSync, writeFileSync, renameSync, rmSync } from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'
import { screen, type Rectangle } from 'electron'

type InitialBounds = Partial<Rectangle> & Pick<Rectangle, 'width' | 'height'>
type Window = {
  getNativeWindowHandle(): Buffer
  getBounds(): Rectangle
  isMaximized(): boolean
  isMinimized(): boolean
  isFullScreen(): boolean
  on(event: 'move' | 'resize' | 'close' | 'closed', listener: () => void): unknown
}
type Geometry = {
  logical: Rectangle
  physical: Rectangle
  displayBounds: Rectangle
  physicalDisplayBounds: Rectangle
}
type NativeSnapshot = {
  physical: Rectangle
  physicalDisplayBounds: Rectangle
  displayBounds: Rectangle
}
// `require` is reserved by the Electron bundle's injected CJS shim.
const windowsRequire = createRequire(import.meta.url)

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function rectangle(value: unknown): Rectangle | undefined {
  if (!object(value)) return undefined
  const { x, y, width, height } = value
  if (
    typeof x !== 'number' ||
    typeof y !== 'number' ||
    typeof width !== 'number' ||
    typeof height !== 'number' ||
    ![x, y, width, height, x + width, y + height].every(
      n => Number.isFinite(n) && Math.abs(n) < 2_147_483_647
    ) ||
    width <= 0 ||
    height <= 0
  )
    return undefined
  return { x, y, width, height }
}

function equal(a: Rectangle, b: Rectangle): boolean {
  return a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height
}

function readState(file: string): Record<string, unknown> | undefined {
  try {
    const value: unknown = JSON.parse(readFileSync(file, 'utf8').replace(/^\uFEFF/u, ''))
    return object(value) ? value : undefined
  } catch (_: unknown) {
    return undefined
  }
}

function geometry(state: Record<string, unknown> | undefined): Geometry | undefined {
  if (!state) return undefined
  const value = state.nativeGeometry
  if (!object(value)) return undefined
  const parsed = ['logical', 'physical', 'displayBounds', 'physicalDisplayBounds'].map(key =>
    rectangle(value[key])
  )
  if (parsed.some(rect => rect === undefined)) return undefined
  const [logical, physical, displayBounds, physicalDisplayBounds] = parsed as [
    Rectangle,
    Rectangle,
    Rectangle,
    Rectangle,
  ]
  const saved = rectangle(state)
  const display = rectangle(state.displayBounds)
  if (!saved || !display) return undefined
  if (!equal(logical, saved) || !equal(displayBounds, display)) return undefined
  if (![physical, physicalDisplayBounds].every(rect => Object.values(rect).every(Number.isInteger)))
    return undefined
  return { logical, physical, displayBounds, physicalDisplayBounds }
}

function withinDisplay(physical: Rectangle, display: Rectangle): boolean {
  // Allow invisible resize frames at display edges, while refusing corrupt
  // metadata that would move a window off screen or allocate an enormous one.
  return (
    physical.x >= display.x - 64 &&
    physical.y >= display.y - 64 &&
    physical.x + physical.width <= display.x + display.width + 64 &&
    physical.y + physical.height <= display.y + display.height + 64
  )
}

function restorable(
  state: Record<string, unknown> | undefined,
  initial: InitialBounds,
  current: NativeSnapshot | undefined
): Geometry | undefined {
  const saved = geometry(state)
  const requested = rectangle(initial)
  if (!saved || !requested || !current) return undefined
  if (
    !equal(saved.logical, requested) ||
    !equal(saved.displayBounds, current.displayBounds) ||
    !equal(saved.physicalDisplayBounds, current.physicalDisplayBounds)
  )
    return undefined
  return withinDisplay(saved.physical, current.physicalDisplayBounds) ? saved : undefined
}

function nativeWindows() {
  const koffi = windowsRequire('koffi') as typeof import('koffi')
  const library = koffi.load('user32.dll')
  const getRect = library.func('int __stdcall GetWindowRect(uintptr_t hwnd, void *rect)') as (
    handle: bigint | number,
    output: Buffer
  ) => number
  const getMonitor = library.func(
    'uintptr_t __stdcall MonitorFromWindow(uintptr_t hwnd, uint32_t flags)'
  ) as (handle: bigint | number, flags: number) => bigint | number
  const getInfo = library.func('int __stdcall GetMonitorInfoW(uintptr_t monitor, void *info)') as (
    handle: bigint | number,
    output: Buffer
  ) => number
  const setPosition = library.func(
    'int __stdcall SetWindowPos(uintptr_t hwnd, uintptr_t after, int x, int y, int width, int height, uint32_t flags)'
  ) as (
    handle: bigint | number,
    after: number,
    x: number,
    y: number,
    width: number,
    height: number,
    flags: number
  ) => number
  const handle = (window: Window) => {
    const bytes = window.getNativeWindowHandle()
    if (bytes.length !== 4 && bytes.length !== 8) throw new Error('Invalid window handle')
    return bytes.length === 8 ? bytes.readBigUInt64LE() : bytes.readUInt32LE()
  }
  const rect = (bytes: Buffer, offset: number): Rectangle => ({
    x: bytes.readInt32LE(offset),
    y: bytes.readInt32LE(offset + 4),
    width: bytes.readInt32LE(offset + 8) - bytes.readInt32LE(offset),
    height: bytes.readInt32LE(offset + 12) - bytes.readInt32LE(offset + 4),
  })
  return {
    read(window: Window): NativeSnapshot | undefined {
      const hwnd = handle(window)
      const windowRect = Buffer.alloc(16)
      const monitorInfo = Buffer.alloc(40)
      monitorInfo.writeUInt32LE(40)
      const monitor = getMonitor(hwnd, 2)
      if (!monitor || !getRect(hwnd, windowRect) || !getInfo(monitor, monitorInfo)) return undefined
      const physical = rectangle(rect(windowRect, 0))
      const physicalDisplayBounds = rectangle(rect(monitorInfo, 4))
      const displayBounds = rectangle(screen.getDisplayMatching(window.getBounds()).bounds)
      return physical && physicalDisplayBounds && displayBounds
        ? { physical, physicalDisplayBounds, displayBounds }
        : undefined
    },
    restore(window: Window, bounds: Rectangle): boolean {
      return (
        setPosition(handle(window), 0, bounds.x, bounds.y, bounds.width, bounds.height, 0x14) !== 0
      )
    },
  }
}

function relocate(bounds: Rectangle, from: Rectangle, to: Rectangle): Rectangle {
  const width = Math.min(bounds.width, to.width)
  const height = Math.min(bounds.height, to.height)
  return {
    x: Math.min(to.x + to.width - width, Math.max(to.x, to.x + bounds.x - from.x)),
    y: Math.min(to.y + to.height - height, Math.max(to.y, to.y + bounds.y - from.y)),
    width,
    height,
  }
}

function attachGeometry(
  state: Record<string, unknown>,
  logical: Rectangle,
  display: Rectangle,
  latest: NativeSnapshot & { logical: Rectangle }
): void {
  if (equal(display, latest.displayBounds)) {
    state.nativeGeometry = { ...latest, logical }
  } else {
    // The legacy manager keeps A's normal bounds when a maximized window moves
    // to B. Keep its mode flags, relocate the normal DIPs, and discard A's exact
    // pixels. A normal-window capture on B supplies precise metadata next time.
    Object.assign(state, relocate(logical, latest.displayBounds, display))
    delete state.nativeGeometry
  }
}

/** Keep precise physical placement alongside electron-window-state's integer
 * DIPs. The logical/display signature prevents stale metadata from overriding
 * a file written by an older build, an explicit display choice or new layout.
 * Register after the manager so its closed handler writes the core fields first.
 */
export function manageWindowsWindowState(
  window: Window,
  directory: string,
  initial: InitialBounds,
  manage: () => void,
  platform: NodeJS.Platform = process.platform
): void {
  if (platform !== 'win32') {
    manage()
    return
  }
  const file = path.join(directory, 'window-state.json')
  let native: ReturnType<typeof nativeWindows> | undefined
  let latest: (NativeSnapshot & { logical: Rectangle }) | undefined
  let warned = false
  const warn = () => {
    if (!warned)
      console.warn('[Window placement] Precise placement unavailable; using saved DIP bounds.')
    warned = true
  }
  const capture = () => {
    try {
      if (window.isMaximized() || window.isMinimized() || window.isFullScreen()) return
      const physical = native?.read(window)
      const logical = rectangle(window.getBounds())
      if (physical && logical) latest = { ...physical, logical }
      else warn()
    } catch (_: unknown) {
      warn()
    }
  }
  try {
    native = nativeWindows()
    const current = native.read(window)
    const saved = restorable(readState(file), initial, current)
    if (saved && !native.restore(window, saved.physical)) warn()
    capture()
  } catch (_: unknown) {
    warn()
  }

  // The manager's failure remains the primary failure; ancillary metadata
  // preparation and persistence must never swallow or replay its call.
  manage()
  if (!native) return
  window.on('move', capture)
  window.on('resize', capture)
  window.on('close', capture)
  window.on('closed', () => {
    const state = readState(file)
    const logical = rectangle(state)
    const display = rectangle(state?.displayBounds)
    if (!state || !logical || !display || !latest || !equal(logical, latest.logical)) return
    attachGeometry(state, logical, display, latest)
    const temporary = `${file}.${process.pid}.native.tmp`
    try {
      writeFileSync(temporary, JSON.stringify(state), { encoding: 'utf8', mode: 0o600 })
      renameSync(temporary, file)
    } catch (_: unknown) {
      warn()
      try {
        rmSync(temporary, { force: true })
      } catch (_: unknown) {
        warn()
      }
    }
  })
}
