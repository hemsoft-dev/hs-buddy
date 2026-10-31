import { cleanup, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { axe } from '../../test/axe-helper'

// Mock terminalSessions before importing the component
vi.mock('./terminalSessions', () => ({
  getSessionId: vi.fn(),
  setSessionId: vi.fn(),
  removeSession: vi.fn(),
  setTerminalPasteHandler: vi.fn(),
  removeTerminalPasteHandler: vi.fn(),
}))

// Mock xterm.js
const mockWrite = vi.fn()
const mockWriteln = vi.fn()
const mockPaste = vi.fn()
const mockOpen = vi.fn()
const mockDispose = vi.fn()
const mockLoadAddon = vi.fn()
const mockOnData = vi.fn((_cb: (data: string) => void) => ({ dispose: vi.fn() }))
const mockOnCursorMove = vi.fn((_cb: () => void) => ({ dispose: vi.fn() }))
const mockOnRender = vi.fn((_cb: () => void) => ({ dispose: vi.fn() }))

vi.mock('@xterm/xterm', () => {
  const TerminalClass = vi.fn(function (this: Record<string, unknown>) {
    this.write = mockWrite
    this.writeln = mockWriteln
    this.paste = mockPaste
    this.open = mockOpen
    this.dispose = mockDispose
    this.loadAddon = mockLoadAddon
    this.onData = mockOnData
    this.onCursorMove = mockOnCursorMove
    this.onRender = mockOnRender
    this.options = {}
    this.unicode = { activeVersion: '6' }
    this.buffer = { active: { cursorY: 0 } }
  })
  return { Terminal: TerminalClass }
})

const mockFit = vi.fn()
const mockProposeDimensions = vi.fn(() => ({ cols: 120, rows: 30 }))

vi.mock('@xterm/addon-fit', () => {
  const FitAddonClass = vi.fn(function (this: Record<string, unknown>) {
    this.fit = mockFit
    this.proposeDimensions = mockProposeDimensions
  })
  return { FitAddon: FitAddonClass }
})

vi.mock('@xterm/addon-unicode11', () => {
  const Unicode11AddonClass = vi.fn()
  return { Unicode11Addon: Unicode11AddonClass }
})

vi.mock('@xterm/xterm/css/xterm.css', () => ({}))
vi.mock('./TerminalPane.css', () => ({}))

const mockInvoke = vi.fn()
const mockSend = vi.fn()
const mockOn = vi.fn()
const mockOff = vi.fn()

const mockSpawn = vi.fn()
const mockAttach = vi.fn()
const mockTerminalWrite = vi.fn()
const mockTerminalResize = vi.fn()

let mockResizeObserverCallback: (() => void) | null = null
const mockResizeObserverDisconnect = vi.fn()

// Mock ResizeObserver globally with a proper constructor
vi.stubGlobal('ResizeObserver', function MockResizeObserver(this: unknown, callback: () => void) {
  mockResizeObserverCallback = callback
  return {
    observe: vi.fn(),
    unobserve: vi.fn(),
    disconnect: mockResizeObserverDisconnect,
  }
})

// Mock document.fonts (not available in happy-dom)
if (!document.fonts) {
  Object.defineProperty(document, 'fonts', {
    value: { load: vi.fn().mockResolvedValue([]) },
    configurable: true,
  })
}

import {
  getSessionId,
  setSessionId,
  removeSession,
  setTerminalPasteHandler,
  removeTerminalPasteHandler,
} from './terminalSessions'
import { TerminalPane } from './TerminalPane'

describe('TerminalPane', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockOn.mockImplementation((channel, listener) => () => mockOff(channel, listener))
    vi.useFakeTimers()
    mockResizeObserverCallback = null
    Object.defineProperty(window, 'ipcRenderer', {
      configurable: true,
      value: {
        invoke: mockInvoke,
        send: mockSend,
        on: mockOn,
        off: mockOff,
      },
    })
    Object.defineProperty(window, 'terminal', {
      configurable: true,
      value: {
        spawn: mockSpawn,
        attach: mockAttach,
        write: mockTerminalWrite,
        resize: mockTerminalResize,
      },
    })

    // Default: no existing session, successful spawn
    vi.mocked(getSessionId).mockReturnValue(undefined)
    mockSpawn.mockResolvedValue({ success: true, sessionId: 'new-sess-123' })
    mockAttach.mockResolvedValue({ success: true })
  })

  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it('renders the terminal pane container', () => {
    const { container } = render(<TerminalPane viewKey="test-key" />)
    expect(container.querySelector('.terminal-pane')).toBeInTheDocument()
  })

  it('has no accessibility violations', async () => {
    vi.useRealTimers()
    const { container } = render(<TerminalPane viewKey="test-key" />)
    const results = await axe(container)
    expect(results).toHaveNoViolations()
  })

  it('registers and unregisters terminal paste handler for the view', () => {
    const { unmount } = render(<TerminalPane viewKey="test-key" />)

    expect(setTerminalPasteHandler).toHaveBeenCalledWith('test-key', expect.any(Function))

    unmount()
    expect(removeTerminalPasteHandler).toHaveBeenCalledWith('test-key')
  })

  it('routes the registered paste handler through xterm paste', () => {
    render(<TerminalPane viewKey="test-key" />)

    const pasteHandler = vi.mocked(setTerminalPasteHandler).mock.calls[0]?.[1]
    expect(pasteHandler).toBeDefined()

    pasteHandler?.('echo hi')
    expect(mockPaste).toHaveBeenCalledWith('echo hi')
  })

  it('spawns a new PTY session when no existing session', async () => {
    render(<TerminalPane viewKey="test-key" cwd="/test/path" />)

    // Wait for the async initSession to complete
    await vi.waitFor(() => {
      expect(mockSpawn).toHaveBeenCalledWith(
        expect.objectContaining({
          cwd: '/test/path',
        })
      )
    })

    expect(setSessionId).toHaveBeenCalledWith('test-key', 'new-sess-123')
  })

  it('passes startupCommand to spawn', async () => {
    render(<TerminalPane viewKey="test-key" cwd="/test" startupCommand="gh copilot" />)

    await vi.waitFor(() => {
      expect(mockSpawn).toHaveBeenCalledWith(
        expect.objectContaining({
          startupCommand: 'gh copilot',
        })
      )
    })
  })

  it('reconnects to existing session via terminal:attach', async () => {
    vi.mocked(getSessionId).mockReturnValue('existing-sess')
    mockAttach.mockResolvedValue({ success: true, buffer: 'buffered output', alive: true })

    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockAttach).toHaveBeenCalledWith('existing-sess')
    })

    expect(mockWrite).toHaveBeenCalledWith('buffered output')
  })

  it('shows exit message when reconnecting to dead session', async () => {
    vi.mocked(getSessionId).mockReturnValue('dead-sess')
    mockAttach.mockResolvedValue({ success: true, buffer: '', alive: false })

    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockAttach).toHaveBeenCalledWith('dead-sess')
    })

    expect(mockWriteln).toHaveBeenCalledWith('\r\n\x1b[90m[Process has exited]\x1b[0m')
  })

  it('spawns fresh session when attach fails', async () => {
    vi.mocked(getSessionId).mockReturnValue('stale-sess')
    mockAttach.mockResolvedValueOnce({ success: false, error: 'gone' })
    mockSpawn.mockResolvedValueOnce({ success: true, sessionId: 'fresh-sess' })

    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(removeSession).toHaveBeenCalledWith('test-key')
    })

    await vi.waitFor(() => {
      expect(mockSpawn).toHaveBeenCalledWith(expect.any(Object))
    })
  })

  it('handles attach failure gracefully after successful spawn', async () => {
    mockSpawn.mockResolvedValue({ success: true, sessionId: 'new-sess-456' })
    mockAttach.mockResolvedValue({ success: false, error: 'attach failed' })

    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockAttach).toHaveBeenCalledWith('new-sess-456')
    })

    // Session is still set even though attach failed (terminal is usable via IPC data events)
    expect(setSessionId).toHaveBeenCalledWith('test-key', 'new-sess-456')
  })

  it('shows error when spawn fails', async () => {
    mockSpawn.mockResolvedValue({ success: false, error: 'No PTY available' })

    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockWriteln).toHaveBeenCalledWith(
        expect.stringContaining('Failed to spawn terminal: No PTY available')
      )
    })
  })

  it('shows generic error when spawn fails without message', async () => {
    mockSpawn.mockResolvedValue({ success: false })

    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockWriteln).toHaveBeenCalledWith(expect.stringContaining('Unknown error'))
    })
  })

  it('registers IPC listeners for terminal:data and terminal:exit after init', async () => {
    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockOn).toHaveBeenCalledWith('terminal:data', expect.any(Function))
      expect(mockOn).toHaveBeenCalledWith('terminal:exit', expect.any(Function))
      expect(mockOn).toHaveBeenCalledWith('terminal:cwd-changed', expect.any(Function))
    })
  })

  it('routes CWD events only to the matching live pane and unsubscribes its exact handler', async () => {
    const firstCwd = vi.fn()
    const secondCwd = vi.fn()
    mockSpawn.mockResolvedValueOnce({ success: true, sessionId: 'first-session' })
    mockSpawn.mockResolvedValueOnce({ success: true, sessionId: 'second-session' })
    const first = render(<TerminalPane viewKey="first" onCwdChange={firstCwd} />)
    render(<TerminalPane viewKey="second" onCwdChange={secondCwd} />)
    await vi.waitFor(() => {
      expect(mockOn.mock.calls.filter(call => call[0] === 'terminal:cwd-changed')).toHaveLength(2)
    })
    const handlers = mockOn.mock.calls
      .filter(call => call[0] === 'terminal:cwd-changed')
      .map(call => call[1])
    for (const handler of handlers) handler(null, 'unrelated-session', '/unrelated')
    expect(firstCwd).not.toHaveBeenCalled()
    expect(secondCwd).not.toHaveBeenCalled()
    for (const handler of handlers) handler(null, 'first-session', '/first/repo')
    expect(firstCwd).toHaveBeenCalledExactlyOnceWith('/first/repo')
    expect(secondCwd).not.toHaveBeenCalled()
    first.unmount()
    expect(mockOff).toHaveBeenCalledWith('terminal:cwd-changed', handlers[0])
    expect(mockOff).not.toHaveBeenCalledWith('terminal:cwd-changed', handlers[1])
    handlers[1](null, 'first-session', '/retired')
    handlers[1](null, 'second-session', '/second/repo')
    expect(firstCwd).toHaveBeenCalledTimes(1)
    expect(secondCwd).toHaveBeenCalledExactlyOnceWith('/second/repo')
  })

  it('accepts matching CWD events without a callback and removes the listener on unmount', async () => {
    const { unmount } = render(<TerminalPane viewKey="no-cwd-callback" />)
    await vi.waitFor(() => {
      expect(mockOn).toHaveBeenCalledWith('terminal:cwd-changed', expect.any(Function))
    })
    const handler = mockOn.mock.calls.find(call => call[0] === 'terminal:cwd-changed')![1]
    expect(() => handler(null, 'new-sess-123', '/repo')).not.toThrow()
    expect(mockSpawn).toHaveBeenCalledTimes(1)
    unmount()
    expect(mockOff).toHaveBeenCalledWith('terminal:cwd-changed', handler)
  })

  it('uses the subscription disposers instead of passing callbacks across the bridge again', async () => {
    const dispose = vi.fn()
    mockOn.mockReturnValue(dispose)
    const { unmount } = render(<TerminalPane viewKey="disposer-ownership" />)
    await vi.advanceTimersByTimeAsync(1_000)
    expect(mockOn).toHaveBeenCalledTimes(3)
    unmount()
    expect(dispose).toHaveBeenCalledTimes(3)
    expect(mockOff).not.toHaveBeenCalled()
  })

  it('removes IPC listeners on unmount', async () => {
    const { unmount } = render(<TerminalPane viewKey="test-key" />)

    // Wait for deferred listener registration before unmounting
    await vi.waitFor(() => {
      expect(mockOn).toHaveBeenCalledWith('terminal:data', expect.any(Function))
    })
    unmount()

    expect(mockOff).toHaveBeenCalledWith('terminal:data', expect.any(Function))
    expect(mockOff).toHaveBeenCalledWith('terminal:exit', expect.any(Function))
    expect(mockOff).toHaveBeenCalledWith('terminal:cwd-changed', expect.any(Function))
  })

  it('disposes terminal on unmount', () => {
    const { unmount } = render(<TerminalPane viewKey="test-key" />)
    unmount()
    expect(mockDispose).toHaveBeenCalled()
  })

  it('forwards keystroke data via fire-and-forget send', async () => {
    render(<TerminalPane viewKey="test-key" />)

    // Wait for session to be established
    await vi.waitFor(() => {
      expect(mockSpawn).toHaveBeenCalledWith(expect.any(Object))
    })

    // Simulate the onData callback being fired with user input
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const onDataCallback = (mockOnData.mock.calls as any)[0][0] as (data: string) => void
    expect(onDataCallback).toBeDefined()
    onDataCallback('hello')

    expect(mockTerminalWrite).toHaveBeenCalledWith('new-sess-123', 'hello')
  })

  it('does not forward keystrokes when no session is active', () => {
    // Make spawn return no session
    mockSpawn.mockResolvedValue({ success: false })

    render(<TerminalPane viewKey="test-key" />)

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const onDataCallback = (mockOnData.mock.calls as any)[0][0] as (data: string) => void
    expect(onDataCallback).toBeDefined()
    onDataCallback('test')

    // write should not be called (no session ID)
    expect(mockTerminalWrite).not.toHaveBeenCalled()
  })

  it('sends resize events when dimensions change', async () => {
    render(<TerminalPane viewKey="test-key" />)

    // Wait for session to be established
    await vi.waitFor(() => {
      expect(mockSpawn).toHaveBeenCalledWith(expect.any(Object))
    })

    // Trigger the ResizeObserver callback
    const resizeCallback = mockResizeObserverCallback
    expect(resizeCallback).toBeDefined()

    // Fire the resize callback
    resizeCallback!()

    // Advance timers to trigger the debounced resize
    await vi.advanceTimersByTimeAsync(150)

    expect(mockFit).toHaveBeenCalled()
    expect(mockTerminalResize).toHaveBeenCalledWith('new-sess-123', 120, 30)
  })

  it('deduplicates resize events when dimensions do not change', async () => {
    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockSpawn).toHaveBeenCalledWith(expect.any(Object))
    })

    const resizeCallback = mockResizeObserverCallback!

    // First resize
    resizeCallback()
    await vi.advanceTimersByTimeAsync(150)
    expect(mockTerminalResize).toHaveBeenCalledWith('new-sess-123', 120, 30)

    mockTerminalResize.mockClear()

    // Second resize with same dimensions — should not send
    resizeCallback()
    await vi.advanceTimersByTimeAsync(150)
    expect(mockTerminalResize).not.toHaveBeenCalled()
  })

  it('calls onExit callback when session exits', async () => {
    const onExit = vi.fn()
    render(<TerminalPane viewKey="test-key" onExit={onExit} />)

    // Wait for session + deferred listener registration
    await vi.waitFor(() => {
      expect(mockOn).toHaveBeenCalledWith('terminal:exit', expect.any(Function))
    })

    // Simulate terminal:exit IPC event
    const exitHandler = mockOn.mock.calls.find(c => c[0] === 'terminal:exit')?.[1]
    expect(exitHandler).toBeDefined()

    // Trigger with matching session ID
    exitHandler(null, 'new-sess-123', 0)

    expect(mockWriteln).toHaveBeenCalledWith(
      expect.stringContaining('[Process exited with code 0]')
    )
    expect(onExit).toHaveBeenCalledWith(0)
  })

  it('writes PTY data to terminal when session IDs match', async () => {
    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockOn).toHaveBeenCalledWith('terminal:data', expect.any(Function))
    })

    const dataHandler = mockOn.mock.calls.find(c => c[0] === 'terminal:data')?.[1]
    expect(dataHandler).toBeDefined()

    dataHandler(null, 'new-sess-123', 'hello world')
    expect(mockWrite).toHaveBeenCalledWith('hello world')
  })

  it('ignores PTY data for non-matching session IDs', async () => {
    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockOn).toHaveBeenCalledWith('terminal:data', expect.any(Function))
    })

    mockWrite.mockClear()
    const dataHandler = mockOn.mock.calls.find(c => c[0] === 'terminal:data')?.[1]
    dataHandler(null, 'different-session', 'should not appear')
    expect(mockWrite).not.toHaveBeenCalled()
  })

  it('writes error to terminal when initSession throws', async () => {
    mockSpawn.mockRejectedValue(new Error('IPC channel destroyed'))

    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockWriteln).toHaveBeenCalledWith(
        expect.stringContaining('[Failed to initialize terminal session]')
      )
    })
  })

  it('replays buffered output from post-spawn attach', async () => {
    mockSpawn.mockResolvedValueOnce({ success: true, sessionId: 'sess-1', cwd: '/resolved/path' })
    mockAttach.mockResolvedValueOnce({ success: true, buffer: 'initial output', cursor: 7 })

    render(<TerminalPane viewKey="test-key" cwd="/test" />)

    await vi.waitFor(() => {
      expect(mockAttach).toHaveBeenCalledWith('sess-1')
    })

    expect(mockWrite).toHaveBeenCalledWith('initial output')
  })

  it('skips data events with seq <= attach cursor to prevent duplicates', async () => {
    vi.mocked(getSessionId).mockReturnValue('existing-sess')
    mockAttach.mockResolvedValue({
      success: true,
      buffer: 'buffered output',
      cursor: 5,
      alive: true,
    })

    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockOn).toHaveBeenCalledWith('terminal:data', expect.any(Function))
    })

    mockWrite.mockClear()
    const dataHandler = mockOn.mock.calls.find(c => c[0] === 'terminal:data')?.[1]
    expect(dataHandler).toBeDefined()

    // Events with seq <= 5 (the attach cursor) should be skipped
    dataHandler(null, 'existing-sess', 'old data 1', 3)
    dataHandler(null, 'existing-sess', 'old data 2', 5)
    expect(mockWrite).not.toHaveBeenCalled()

    // Events with seq > 5 should pass through
    dataHandler(null, 'existing-sess', 'new data', 6)
    expect(mockWrite).toHaveBeenCalledWith('new data')
  })

  it('applies cursor-row highlight on cursor move', async () => {
    // Make mockOpen create .xterm-rows DOM structure inside container
    mockOpen.mockImplementation((el: HTMLElement) => {
      const rowContainer = document.createElement('div')
      rowContainer.className = 'xterm-rows'
      for (let i = 0; i < 5; i++) {
        rowContainer.appendChild(document.createElement('div'))
      }
      el.appendChild(rowContainer)
    })

    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockOnCursorMove).toHaveBeenCalled()
    })

    // Capture the cursor-move callback
    const cursorMoveCallback = mockOnCursorMove.mock.calls[0]![0]

    // Trigger cursor move — cursorY defaults to 0
    cursorMoveCallback()

    const terminalPane = document.querySelector('.terminal-pane')!
    const rowContainer = terminalPane.querySelector('.xterm-rows')!
    expect(rowContainer.children[0].classList.contains('xterm-cursor-row')).toBe(true)
  })

  it('moves cursor-row highlight when cursor position changes', async () => {
    mockOpen.mockImplementation((el: HTMLElement) => {
      const rowContainer = document.createElement('div')
      rowContainer.className = 'xterm-rows'
      for (let i = 0; i < 5; i++) {
        rowContainer.appendChild(document.createElement('div'))
      }
      el.appendChild(rowContainer)
    })

    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockOnCursorMove).toHaveBeenCalled()
    })

    const cursorMoveCallback = mockOnCursorMove.mock.calls[0]![0]

    // First call — highlight row 0
    cursorMoveCallback()

    const terminalPane = document.querySelector('.terminal-pane')!
    const rowContainer = terminalPane.querySelector('.xterm-rows')!
    expect(rowContainer.children[0].classList.contains('xterm-cursor-row')).toBe(true)

    // Simulate cursor moving to row 2 by updating the buffer mock
    // Access the Terminal instance's buffer through the constructor mock
    const TerminalMock = (await import('@xterm/xterm')).Terminal as unknown as ReturnType<
      typeof vi.fn
    >
    const termInstance = TerminalMock.mock.instances[0] as {
      buffer: { active: { cursorY: number } }
    }
    termInstance.buffer.active.cursorY = 2

    // Trigger cursor move again
    cursorMoveCallback()

    // Previous highlight removed, new one applied
    expect(rowContainer.children[0].classList.contains('xterm-cursor-row')).toBe(false)
    expect(rowContainer.children[2].classList.contains('xterm-cursor-row')).toBe(true)
  })

  it('handles missing row container gracefully in cursor highlight', async () => {
    // Explicitly ensure mockOpen does NOT create .xterm-rows
    mockOpen.mockImplementation(() => {})

    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockOnCursorMove).toHaveBeenCalled()
    })

    const cursorMoveCallback = mockOnCursorMove.mock.calls[0]![0]

    // Should not throw when .xterm-rows doesn't exist
    expect(() => cursorMoveCallback()).not.toThrow()
  })

  it('handles cursor position beyond row count gracefully', async () => {
    mockOpen.mockImplementation((el: HTMLElement) => {
      const rowContainer = document.createElement('div')
      rowContainer.className = 'xterm-rows'
      // Only 2 rows, but cursorY will be 10
      for (let i = 0; i < 2; i++) {
        rowContainer.appendChild(document.createElement('div'))
      }
      el.appendChild(rowContainer)
    })

    render(<TerminalPane viewKey="test-key" />)

    await vi.waitFor(() => {
      expect(mockOnCursorMove).toHaveBeenCalled()
    })

    // Set cursorY beyond available rows
    const TerminalMock = (await import('@xterm/xterm')).Terminal as unknown as ReturnType<
      typeof vi.fn
    >
    const termInstance = TerminalMock.mock.instances[0] as {
      buffer: { active: { cursorY: number } }
    }
    termInstance.buffer.active.cursorY = 10

    const cursorMoveCallback = mockOnCursorMove.mock.calls[0]![0]

    // Should not throw — row will be undefined, if(row) skips
    expect(() => cursorMoveCallback()).not.toThrow()

    const terminalPane = document.querySelector('.terminal-pane')!
    const rowContainer = terminalPane.querySelector('.xterm-rows')!
    // No row should have the highlight class
    expect(rowContainer.querySelector('.xterm-cursor-row')).toBeNull()
  })
})
