import fixture from './matching-fixture.json'
import type { Page } from 'puppeteer-core'
import { type ProcessKind, type ScenarioMetrics } from './electron-memory-model'
import { collectScenario, type LaunchedRuntime } from './electron-memory-runtime'

interface LifecycleOptions {
  settleMs: number
  navigationCycles: number
}

export async function waitWithProgress(milliseconds: number, label: string): Promise<void> {
  if (milliseconds === 0) return
  const started = Date.now()
  while (Date.now() - started < milliseconds) {
    const remaining = milliseconds - (Date.now() - started)
    await new Promise(resolve => setTimeout(resolve, Math.min(60_000, remaining)))
    const elapsed = Math.min(milliseconds, Date.now() - started)
    console.log(`${label}: ${Math.round(elapsed / 1_000)}s/${Math.round(milliseconds / 1_000)}s`)
  }
}

export async function navigate(page: Page, viewId: string): Promise<void> {
  await page.evaluate(nextViewId => {
    window.dispatchEvent(new CustomEvent('app:navigate', { detail: { viewId: nextViewId } }))
  }, viewId)
  await new Promise(resolve => setTimeout(resolve, 500))
}

export async function closeNonDashboardTabs(page: Page): Promise<void> {
  await page.evaluate(() => {
    const dashboard = [...document.querySelectorAll<HTMLButtonElement>('.tab-select')].find(
      button => button.textContent?.trim() === 'Dashboard'
    )
    dashboard?.click()
  })
  while (
    await page.evaluate(() => {
      const closeButton = document.querySelector<HTMLButtonElement>(
        '.tab-close:not([aria-label="Close Dashboard"])'
      )
      closeButton?.click()
      return Boolean(closeButton)
    })
  ) {
    await new Promise(resolve => setTimeout(resolve, 100))
  }
}

export async function runNavigationScenario(page: Page, cycles: number): Promise<void> {
  const views = fixture.routes
  for (let cycle = 0; cycle < cycles; cycle += 1) {
    for (const view of views) await navigate(page, view)
  }
  await closeNonDashboardTabs(page)
}

async function openTerminal(page: Page): Promise<void> {
  await page.evaluate(() =>
    document.querySelector<HTMLButtonElement>('button[aria-label^="Toggle Terminal"]')?.click()
  )
  await page.waitForSelector('.terminal-panel', { visible: true })
  const automaticTerminal = await page
    .waitForSelector('.xterm', { visible: true, timeout: 30_000 })
    .catch(() => null)
  if (!automaticTerminal) {
    await page.evaluate(() =>
      document.querySelector<HTMLButtonElement>('button[aria-label="New Terminal"]')?.click()
    )
    await page.waitForSelector('.xterm', { visible: true, timeout: 30_000 })
  }
  const textarea = await page.$('.xterm-helper-textarea')
  if (textarea) {
    await textarea.focus()
    await page.keyboard.type('echo buddy-memory-probe')
    await page.keyboard.press('Enter')
  }
  await new Promise(resolve => setTimeout(resolve, 1_000))
}

async function closeTerminal(page: Page): Promise<void> {
  while (
    await page.evaluate(() => {
      const closeButton = document.querySelector<HTMLButtonElement>('.terminal-panel-tab-close')
      closeButton?.click()
      return Boolean(closeButton)
    })
  ) {
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  await new Promise(resolve => setTimeout(resolve, 1_000))
}

async function openBrowser(page: Page): Promise<void> {
  const route = `browser:${encodeURIComponent('https://example.com')}|${encodeURIComponent('Memory Probe')}`
  await navigate(page, route)
  await page.waitForSelector('webview', { timeout: 30_000 })
  await new Promise(resolve => setTimeout(resolve, 3_000))
}

async function closeActiveTab(page: Page): Promise<void> {
  await page.evaluate(() =>
    document.querySelector<HTMLButtonElement>('.tab.active .tab-close')?.click()
  )
  await closeNonDashboardTabs(page)
}

export function requireNewProcessKind(
  scenarioName: string,
  baseline: ScenarioMetrics,
  scenario: ScenarioMetrics,
  kind: ProcessKind
): void {
  const baselinePids = new Set(baseline.processes.map(process => process.pid))
  if (
    !scenario.processes.some(process => process.kind === kind && !baselinePids.has(process.pid))
  ) {
    throw new Error(`${scenarioName} did not observe a new ${kind} process`)
  }
}

function requireNewProcessKindWhenSupported(
  scenarioName: string,
  baseline: ScenarioMetrics,
  scenario: ScenarioMetrics,
  kind: ProcessKind
): void {
  if (process.platform === 'win32') {
    requireNewProcessKind(scenarioName, baseline, scenario, kind)
  }
}

export async function runTerminalLifecycle(
  page: Page,
  runtime: LaunchedRuntime,
  options: LifecycleOptions,
  scenarios: Record<string, ScenarioMetrics>
): Promise<void> {
  await openTerminal(page)
  await closeTerminal(page)
  await waitWithProgress(options.settleMs, 'terminal first cleanup')
  scenarios['terminal-first-cleanup'] = await collectScenario('terminal-first-cleanup', runtime)

  for (let cycle = 1; cycle < options.navigationCycles; cycle += 1) {
    await openTerminal(page)
    await closeTerminal(page)
  }
  await waitWithProgress(options.settleMs, 'terminal post-warmup baseline')
  scenarios['terminal-baseline'] = await collectScenario('terminal-baseline', runtime)

  for (let cycle = 0; cycle < options.navigationCycles; cycle += 1) {
    await openTerminal(page)
    if (cycle === 0) {
      const terminalOpen = await collectScenario('terminal-open', runtime)
      scenarios['terminal-open'] = terminalOpen
      requireNewProcessKindWhenSupported(
        'terminal-open',
        scenarios['terminal-baseline'],
        terminalOpen,
        'spawned-child'
      )
    }
    await closeTerminal(page)
  }
  await waitWithProgress(options.settleMs, 'terminal cleanup')
  scenarios['terminal-cleanup'] = await collectScenario('terminal-cleanup', runtime)
}

export async function runBrowserLifecycle(
  page: Page,
  runtime: LaunchedRuntime,
  options: LifecycleOptions,
  scenarios: Record<string, ScenarioMetrics>
): Promise<void> {
  await openBrowser(page)
  await closeActiveTab(page)
  await waitWithProgress(options.settleMs, 'browser first cleanup')
  scenarios['browser-first-cleanup'] = await collectScenario('browser-first-cleanup', runtime)

  await waitWithProgress(options.settleMs, 'browser post-warmup baseline')
  scenarios['browser-baseline'] = await collectScenario('browser-baseline', runtime)

  for (let cycle = 0; cycle < options.navigationCycles; cycle += 1) {
    await openBrowser(page)
    if (cycle === 0) {
      const browserOpen = await collectScenario(
        'browser-open',
        runtime,
        scenarios['browser-baseline']
      )
      scenarios['browser-open'] = browserOpen
      requireNewProcessKindWhenSupported(
        'browser-open',
        scenarios['browser-baseline'],
        browserOpen,
        'webview'
      )
    }
    await closeActiveTab(page)
  }
  await waitWithProgress(options.settleMs, 'browser cleanup')
  scenarios['browser-cleanup'] = await collectScenario('browser-cleanup', runtime)
}
