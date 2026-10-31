// eslint-disable-next-line @typescript-eslint/triple-slash-reference
/// <reference path="../electron/electron-env.d.ts" />

import { readFile } from 'node:fs/promises'
import path from 'node:path'
import type { ElectronApplication, Page } from '@playwright/test'
import { test, expect, FIXTURE_FILE_CONTENT, FIXTURE_FILE_NAME } from './electron-fixtures'

function terminalProbe(): { command: string; expectedSize: string; marker: string } {
  const marker = '__BUDDY_ELECTRON_TERMINAL_OK__'
  const sizeMarker = '__BUDDY_SIZE__'
  return process.platform === 'win32'
    ? {
        command: `Write-Output '${marker}'; $s=$Host.UI.RawUI.WindowSize; Write-Output "${sizeMarker}$($s.Width)x$($s.Height)"`,
        expectedSize: `${sizeMarker}100x32`,
        marker,
      }
    : {
        command: `echo '${marker}'; printf '${sizeMarker}'; stty size`,
        expectedSize: `${sizeMarker}32 100`,
        marker,
      }
}

async function waitForTerminalOutput(
  page: Page,
  sessionId: string,
  expectedSize: string
): Promise<string> {
  await expect
    .poll(
      async () => {
        const attached = await page.evaluate(id => window.terminal.attach(id), sessionId)
        expect(attached.success, attached.error).toBe(true)
        return attached.buffer ?? ''
      },
      { timeout: 15_000 }
    )
    .toContain(expectedSize)
  const attached = await page.evaluate(id => window.terminal.attach(id), sessionId)
  return attached.buffer ?? ''
}

async function killAndVerifyTerminal(page: Page, sessionId: string): Promise<void> {
  expect(await page.evaluate(id => window.terminal.kill(id), sessionId)).toEqual({ success: true })
  expect(await page.evaluate(id => window.terminal.attach(id), sessionId)).toEqual({
    success: false,
    error: 'Session not found',
  })
}

type ZoomInput = Electron.KeyboardInputEvent | Electron.MouseWheelInputEvent

const zoomModifier = process.platform === 'darwin' ? 'meta' : 'control'

function zoomFactor(app: ElectronApplication): Promise<number> {
  return app.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows()[0].webContents.getZoomFactor()
  )
}

// sendInputEvent takes the same input path as real devices, unlike page-level
// synthesized events, so it reaches before-input-event and zoom-changed.
async function sendZoomInput(app: ElectronApplication, input: ZoomInput): Promise<void> {
  await app.evaluate(({ BrowserWindow }, event) => {
    BrowserWindow.getAllWindows()[0].webContents.sendInputEvent(event)
  }, input)
}

async function resetZoom(app: ElectronApplication, page: Page): Promise<void> {
  await page.evaluate(() => {
    window.ipcRenderer.send('zoom-reset')
  })
  await expect.poll(() => zoomFactor(app)).toBe(1)
}

function ctrlWheel(deltaY: number): Electron.MouseWheelInputEvent {
  return {
    type: 'mouseWheel',
    x: 200,
    y: 200,
    deltaY,
    wheelTicksY: Math.sign(deltaY),
    canScroll: true,
    modifiers: [zoomModifier],
  }
}

test.describe('real Electron renderer-to-main journeys', () => {
  test('persists configuration inside the isolated user-data directory', async ({
    electronHarness,
  }) => {
    const { page, userDataDir } = electronHarness
    const result = await page.evaluate(async () => {
      const saved = await window.ipcRenderer.invoke('config:set-theme', 'light')
      const theme = await window.ipcRenderer.invoke('config:get-theme')
      const storePath = await window.ipcRenderer.invoke('config:get-store-path')
      return { saved, storePath, theme }
    })

    expect(result.saved).toEqual({ success: true })
    expect(result.theme).toBe('light')
    const relativeStorePath = path.relative(userDataDir, result.storePath)
    expect(path.isAbsolute(relativeStorePath)).toBe(false)
    expect(relativeStorePath).not.toMatch(/^\.\.(?:[\\/]|$)/)

    const storedConfig = JSON.parse(await readFile(result.storePath, 'utf8')) as {
      ui?: { theme?: string }
    }
    expect(storedConfig.ui?.theme).toBe('light')
  })

  test('reads deterministic files through the preload filesystem bridge', async ({
    electronHarness,
  }) => {
    const { fixtureRoot, page } = electronHarness
    const result = await page.evaluate(
      async ({ directory, fileName }) => {
        const directoryResult = await window.filesystem.readDir(directory)
        const fileResult = await window.filesystem.readFile(`${directory}/${fileName}`)
        return { directoryResult, fileResult }
      },
      { directory: fixtureRoot.replaceAll('\\', '/'), fileName: FIXTURE_FILE_NAME }
    )

    expect(result.directoryResult.error).toBeUndefined()
    expect(result.directoryResult.entries).toEqual([
      expect.objectContaining({ name: FIXTURE_FILE_NAME, type: 'file' }),
    ])
    expect(result.fileResult).toEqual({
      content: FIXTURE_FILE_CONTENT,
      language: 'plaintext',
      size: Buffer.byteLength(FIXTURE_FILE_CONTENT),
    })
  })

  test('spawns, writes, resizes, and kills a native terminal session', async ({
    electronHarness,
  }) => {
    const { fixtureRoot, page } = electronHarness
    const probe = terminalProbe()
    const spawnResult = await page.evaluate(
      cwd => window.terminal.spawn({ cwd, cols: 80, rows: 24 }),
      fixtureRoot
    )

    expect(spawnResult.success, spawnResult.error).toBe(true)
    expect(spawnResult.sessionId).toEqual(expect.any(String))
    const sessionId = spawnResult.sessionId!

    try {
      await page.evaluate(
        ({ id, terminalCommand }) => {
          window.terminal.resize(id, 100, 32)
          window.terminal.write(id, `${terminalCommand}\r`)
        },
        { id: sessionId, terminalCommand: probe.command }
      )

      expect(await waitForTerminalOutput(page, sessionId, probe.expectedSize)).toContain(
        probe.marker
      )
      await killAndVerifyTerminal(page, sessionId)
    } finally {
      await page.evaluate(id => window.terminal.kill(id), sessionId)
    }
  })
})

test('subscription disposers remove only their captured callback across the real context bridge', async ({
  electronHarness,
}) => {
  const { app, page } = electronHarness
  const channel = 'buddy:test:subscription-disposal'
  await page.evaluate(channel => {
    const probe = { removedHits: 0, liveHits: 0, remove: () => {}, keep: () => {} }
    probe.remove = window.ipcRenderer.on(channel, () => {
      probe.removedHits += 1
    })
    probe.keep = window.ipcRenderer.on(channel, () => {
      probe.liveHits += 1
    })
    Object.assign(window, { __buddySubscriptionProbe: probe })
  }, channel)
  const hits = () =>
    page.evaluate(() => {
      const probe = (
        window as Window & { __buddySubscriptionProbe: { removedHits: number; liveHits: number } }
      ).__buddySubscriptionProbe
      return [probe.removedHits, probe.liveHits]
    })
  const send = () =>
    app.evaluate(({ BrowserWindow }, channel) => {
      BrowserWindow.getAllWindows()[0].webContents.send(channel)
    }, channel)
  try {
    await send()
    await expect.poll(hits).toEqual([1, 1])
    await page.evaluate(() => {
      const probe = (window as Window & { __buddySubscriptionProbe: { remove: () => void } })
        .__buddySubscriptionProbe
      probe.remove()
      probe.remove()
    })
    await send()
    // The live callback acknowledges delivery, so an asynchronous no-op cannot pass.
    await expect.poll(hits).toEqual([1, 2])
  } finally {
    await page.evaluate(() => {
      const target = window as Window & {
        __buddySubscriptionProbe?: { remove: () => void; keep: () => void }
      }
      target.__buddySubscriptionProbe?.remove()
      target.__buddySubscriptionProbe?.keep()
      delete target.__buddySubscriptionProbe
    })
  }
})

test.describe('real Electron window zoom', () => {
  test('zooms the window from Ctrl+=, Ctrl+mouse wheel and the menu channels', async ({
    electronHarness,
  }) => {
    const { app, page } = electronHarness
    try {
      await resetZoom(app, page)
      await sendZoomInput(app, { type: 'keyDown', keyCode: '=', modifiers: [zoomModifier] })
      await expect.poll(() => zoomFactor(app)).toBe(1.1)

      await resetZoom(app, page)
      await sendZoomInput(app, ctrlWheel(120))
      await expect.poll(() => zoomFactor(app)).toBeGreaterThan(1)

      await resetZoom(app, page)
      await sendZoomInput(app, ctrlWheel(-120))
      await expect.poll(() => zoomFactor(app)).toBeLessThan(1)

      await resetZoom(app, page)
      await page.evaluate(() => {
        window.ipcRenderer.send('zoom-in')
      })
      await expect.poll(() => zoomFactor(app)).toBe(1.1)
    } finally {
      await resetZoom(app, page)
    }
  })
})
