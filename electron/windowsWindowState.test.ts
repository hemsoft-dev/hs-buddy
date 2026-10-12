import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import * as fs from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import type { Rectangle } from 'electron'

const ffi = vi.hoisted(() => ({
  getRect: vi.fn(),
  getMonitor: vi.fn(),
  getInfo: vi.fn(),
  setPosition: vi.fn(),
  load: vi.fn(),
  require: vi.fn(),
  display: vi.fn(),
}))
vi.mock('electron', () => ({ screen: { getDisplayMatching: ffi.display } }))
vi.mock('node:module', () => ({ createRequire: () => ffi.require }))
vi.mock('node:fs', async importOriginal => ({
  ...(await importOriginal<typeof import('node:fs')>()),
}))
import { manageWindowsWindowState } from './windowsWindowState'

const logical = { x: 128, y: 90, width: 1484, height: 1042 }
const physical = { x: 120, y: 90, width: 1500, height: 1050 }
const display = { x: 0, y: 0, width: 3440, height: 1440 }
const bridge = { logical, physical, displayBounds: display, physicalDisplayBounds: display }
let directory: string
let file: string
let current: Rectangle
let currentDisplay: Rectangle
let logicalDisplay: Rectangle
let bounds: Rectangle
let handle: Buffer
let mode: 'normal' | 'maximized' | 'minimized' | 'fullScreen'
let events: Map<string, (() => void)[]>

function writeRect(output: Buffer, offset: number, value: Rectangle) {
  for (const [index, n] of [
    value.x,
    value.y,
    value.x + value.width,
    value.y + value.height,
  ].entries())
    output.writeInt32LE(n, offset + index * 4)
}
function saved(extra: Record<string, unknown> = {}) {
  return { ...logical, displayBounds: display, nativeGeometry: bridge, retained: 'keep', ...extra }
}
function save(value: unknown) {
  writeFileSync(file, JSON.stringify(value))
}
function fire(event: string) {
  for (const listener of events.get(event) ?? []) listener()
}
function fixture() {
  return {
    getNativeWindowHandle: () => handle,
    getBounds: () => bounds,
    isMaximized: () => mode === 'maximized',
    isMinimized: () => mode === 'minimized',
    isFullScreen: () => mode === 'fullScreen',
    on(event: string, listener: () => void) {
      events.set(event, [...(events.get(event) ?? []), listener])
    },
  }
}
function bind(
  initial: Partial<Rectangle> = logical,
  manage = () => fixture().on('closed', () => save(saved({ nativeGeometry: undefined, ...bounds })))
) {
  manageWindowsWindowState(
    fixture(),
    directory,
    { width: 1484, height: 1042, ...initial },
    manage,
    'win32'
  )
}

beforeEach(() => {
  vi.resetAllMocks()
  vi.spyOn(console, 'warn').mockImplementation(() => {})
  directory = mkdtempSync(path.join(tmpdir(), 'buddy-window-bridge-'))
  file = path.join(directory, 'window-state.json')
  current = { ...physical }
  currentDisplay = { ...display }
  logicalDisplay = { ...display }
  bounds = { ...logical }
  mode = 'normal'
  events = new Map()
  handle = Buffer.alloc(8)
  handle.writeBigUInt64LE(123n)
  ffi.getMonitor.mockReturnValue(456n)
  ffi.display.mockImplementation(() => ({ bounds: logicalDisplay }))
  ffi.getRect.mockImplementation((_: unknown, output: Buffer) => {
    writeRect(output, 0, current)
    return 1
  })
  ffi.getInfo.mockImplementation((_: unknown, output: Buffer) => {
    expect(output.readUInt32LE()).toBe(40)
    writeRect(output, 4, currentDisplay)
    return 1
  })
  ffi.setPosition.mockImplementation(
    (_: unknown, _after: unknown, x: number, y: number, width: number, height: number) => {
      current = { x, y, width, height }
      return 1
    }
  )
  ffi.load.mockReturnValue({
    func: (signature: string) => {
      if (signature.includes('GetWindowRect')) return ffi.getRect
      if (signature.includes('MonitorFromWindow')) return ffi.getMonitor
      if (signature.includes('GetMonitorInfoW')) return ffi.getInfo
      return ffi.setPosition
    },
  })
  ffi.require.mockReturnValue({ load: ffi.load })
})
afterEach(() => {
  rmSync(directory, { recursive: true, force: true })
  vi.restoreAllMocks()
})

describe('precise Windows restoration', () => {
  it('restores the actual physical rectangle and preserves the manager save order', () => {
    save(saved())
    current = { x: 300, y: 200, width: 1800, height: 1200 }
    bind()
    fire('close')
    fire('closed')
    expect(ffi.setPosition.mock.calls).toEqual([[123n, 0, 120, 90, 1500, 1050, 0x14]])
    const disk = JSON.parse(readFileSync(file, 'utf8'))
    expect(disk.nativeGeometry).toEqual(bridge)
    expect(disk.retained).toBe('keep')
  })
  it('supports 32-bit handles without reading beyond their buffer', () => {
    handle = Buffer.alloc(4)
    handle.writeUInt32LE(123)
    save(saved())
    bind()
    expect(ffi.setPosition.mock.calls[0]?.[0]).toBe(123)
  })
  it('does not load native code or alter persistence on another platform', () => {
    const manage = vi.fn()
    manageWindowsWindowState(fixture(), directory, logical, manage, 'linux')
    expect(manage).toHaveBeenCalledOnce()
    expect(ffi.require).not.toHaveBeenCalled()
    expect(events.size).toBe(0)
  })
  it('keeps the original manager exception and never replays it after preparation fails', () => {
    ffi.load.mockImplementation(() => {
      throw new Error('native unavailable')
    })
    const failure = new Error('manager failure')
    const manage = vi.fn(() => {
      throw failure
    })
    expect(() => bind(logical, manage)).toThrow(failure)
    expect(manage).toHaveBeenCalledOnce()
  })
})

describe('stale and corrupt Windows placement', () => {
  it('refuses exact pixels after a DPI change with unchanged physical resolution', () => {
    save(saved())
    logicalDisplay = { ...display, width: 2294, height: 960 }
    bind()
    expect(ffi.setPosition).not.toHaveBeenCalled()
  })
  it.each([null, false, 0, '', [], {}, { nativeGeometry: null }])(
    'ignores malformed or absent records %j',
    value => {
      save(value)
      bind()
      expect(ffi.setPosition).not.toHaveBeenCalled()
    }
  )
  it('ignores a stale logical signature instead of overriding newer Electron bounds', () => {
    save(saved({ x: 500 }))
    bind({ x: 500 })
    expect(ffi.setPosition).not.toHaveBeenCalled()
  })
  it.each([
    { width: 0 },
    { displayBounds: null },
    { nativeGeometry: { ...bridge, logical: null } },
    { nativeGeometry: { ...bridge, physicalDisplayBounds: { ...display, x: 0.5 } } },
  ])('keeps the manager placement when any saved signature is invalid %j', change => {
    save(saved(change))
    const manage = vi.fn()
    bind(logical, manage)
    expect(manage).toHaveBeenCalledOnce()
    expect(ffi.setPosition).not.toHaveBeenCalled()
  })
  it('honors a changed resolved display choice or disconnected display', () => {
    save(saved())
    bind({ x: 600 })
    expect(ffi.setPosition).not.toHaveBeenCalled()
    events.clear()
    currentDisplay = { ...display, width: 1920 }
    bind()
    expect(ffi.setPosition).not.toHaveBeenCalled()
  })
  it.each([
    { physical: { ...physical, x: 20000 } },
    { physical: { ...physical, width: -1 } },
    { physical: { ...physical, x: 0.5 } },
    { physical: { ...physical, height: 2_147_483_647 } },
    { displayBounds: { ...display, width: 100 } },
    { physicalDisplayBounds: { ...display, width: 0 } },
  ])('rejects invalid or off-screen physical metadata %j', change => {
    save(saved({ nativeGeometry: { ...bridge, ...change } }))
    bind()
    expect(ffi.setPosition).not.toHaveBeenCalled()
  })
})

describe('maximized Windows display handoff', () => {
  it.each([
    { x: 3440, y: 0, width: 2195, height: 1235 },
    { x: -1920, y: -240, width: 1920, height: 1080 },
    { x: 0, y: -900, width: 1200, height: 900 },
  ])('relocates retained normal bounds after a maximized move to %j', destination => {
    const manage = vi.fn(() =>
      fixture().on('closed', () =>
        save(
          saved({
            nativeGeometry: undefined,
            displayBounds: destination,
            isMaximized: true,
          })
        )
      )
    )
    bind(logical, manage)
    mode = 'maximized'
    logicalDisplay = destination
    bounds = destination
    fire('move')
    fire('close')
    fire('closed')
    const disk = JSON.parse(readFileSync(file, 'utf8'))
    expect(disk.x).toBe(
      destination.x +
        Math.min(logical.x, destination.width - Math.min(logical.width, destination.width))
    )
    expect(disk.y).toBe(
      destination.y +
        Math.min(logical.y, destination.height - Math.min(logical.height, destination.height))
    )
    expect(disk.width).toBe(Math.min(logical.width, destination.width))
    expect(disk.height).toBe(Math.min(logical.height, destination.height))
    expect(disk.isMaximized).toBe(true)
    expect(disk.retained).toBe('keep')
    expect(disk.nativeGeometry).toBeUndefined()
    expect(manage).toHaveBeenCalledOnce()
  })
  it('does not relocate a core rectangle replaced by another writer', () => {
    const destination = { x: 3440, y: 0, width: 2195, height: 1235 }
    bind(logical, () =>
      fixture().on('closed', () =>
        save(
          saved({
            x: 3500,
            displayBounds: destination,
            nativeGeometry: undefined,
          })
        )
      )
    )
    fire('closed')
    const disk = JSON.parse(readFileSync(file, 'utf8'))
    expect(disk.x).toBe(3500)
    expect(disk.nativeGeometry).toBeUndefined()
  })
})

describe('Windows placement capture and failures', () => {
  it('persists a fresh move with exact OS output and reread disk bytes', () => {
    bind()
    bounds = { x: 1008, y: 200, width: 984, height: 692 }
    current = { x: 1000, y: 200, width: 1000, height: 700 }
    fire('move')
    fire('close')
    fire('closed')
    expect(JSON.parse(readFileSync(file, 'utf8')).nativeGeometry).toEqual({
      logical: bounds,
      physical: current,
      displayBounds: display,
      physicalDisplayBounds: display,
    })
    expect(ffi.getRect.mock.calls.every(call => call[0] === 123n)).toBe(true)
  })
  it.each(['maximized', 'minimized', 'fullScreen'] as const)(
    'keeps the last normal placement while %s',
    value => {
      bind()
      mode = value
      current = { x: 0, y: 0, width: 3440, height: 1440 }
      fire('resize')
      fire('close')
      fire('closed')
      expect(JSON.parse(readFileSync(file, 'utf8')).nativeGeometry.physical).toEqual(physical)
    }
  )
  it('does not attach stale geometry after another writer changes the core state', () => {
    bind(logical, () =>
      fixture().on('closed', () => save(saved({ x: 999, nativeGeometry: undefined })))
    )
    fire('closed')
    expect(JSON.parse(readFileSync(file, 'utf8')).nativeGeometry).toBeUndefined()
  })
  it.each(['rect', 'monitor', 'info', 'handle', 'restore'] as const)(
    'uses the fallback and warns once on %s failure',
    which => {
      save(saved())
      if (which === 'rect') ffi.getRect.mockReturnValue(0)
      if (which === 'monitor') ffi.getMonitor.mockReturnValue(0)
      if (which === 'info') ffi.getInfo.mockReturnValue(0)
      if (which === 'handle') handle = Buffer.alloc(1)
      if (which === 'restore') ffi.setPosition.mockReturnValue(0)
      const manage = vi.fn()
      bind(logical, manage)
      fire('move')
      fire('resize')
      expect(manage).toHaveBeenCalledOnce()
      expect(console.warn).toHaveBeenCalledOnce()
    }
  )
  it('handles missing and corrupt files and writes valid state on the next close', () => {
    writeFileSync(file, '{broken')
    bind()
    fire('closed')
    expect(JSON.parse(readFileSync(file, 'utf8')).nativeGeometry.physical).toEqual(physical)
  })
})

describe('placement disk failure recovery', () => {
  it('retains core state and removes the generated temporary file after rename failure', () => {
    bind()
    vi.spyOn(fs, 'renameSync').mockImplementation(() => {
      throw new Error('rename failed')
    })
    fire('closed')
    const disk = JSON.parse(readFileSync(file, 'utf8'))
    expect(disk).toEqual(saved({ nativeGeometry: undefined }))
    expect(fs.existsSync(`${file}.${process.pid}.native.tmp`)).toBe(false)
    expect(console.warn).toHaveBeenCalledOnce()
  })
  it('keeps core bytes after an observed metadata staging write failure', () => {
    const originalWrite = fs.writeFileSync
    const core = saved({ nativeGeometry: undefined })
    bind(logical, () => fixture().on('closed', () => originalWrite(file, JSON.stringify(core))))
    const write = vi.spyOn(fs, 'writeFileSync').mockImplementation(() => {
      throw new Error('metadata write failed')
    })
    fire('closed')
    expect(write).toHaveBeenCalledOnce()
    expect(write.mock.calls[0]?.[0]).toBe(`${file}.${process.pid}.native.tmp`)
    expect(JSON.parse(readFileSync(file, 'utf8'))).toEqual(core)
    expect(fs.existsSync(`${file}.${process.pid}.native.tmp`)).toBe(false)
    expect(console.warn).toHaveBeenCalledOnce()
  })
  it('records unavailable cleanup without replaying the primary manager', () => {
    const manage = vi.fn(() =>
      fixture().on('closed', () => save(saved({ nativeGeometry: undefined })))
    )
    bind(logical, manage)
    vi.spyOn(fs, 'renameSync').mockImplementation(() => {
      throw new Error('rename failed')
    })
    const originalRemove = fs.rmSync
    const cleanup = vi.spyOn(fs, 'rmSync').mockImplementation(() => {
      throw new Error('cleanup failed')
    })
    fire('closed')
    expect(manage).toHaveBeenCalledOnce()
    expect(JSON.parse(readFileSync(file, 'utf8'))).toEqual(saved({ nativeGeometry: undefined }))
    expect(fs.existsSync(`${file}.${process.pid}.native.tmp`)).toBe(true)
    expect(console.warn).toHaveBeenCalledOnce()
    cleanup.mockRestore()
    originalRemove(`${file}.${process.pid}.native.tmp`)
  })
})
