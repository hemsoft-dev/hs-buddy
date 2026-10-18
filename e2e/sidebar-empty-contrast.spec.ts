import { test, expect, waitForAppReady } from './fixtures'

test.use({
  githubAccounts: [[], { scope: 'test' }],
  video: { mode: 'on', size: { width: 1200, height: 820 } },
})

function channels(color: string): number[] {
  const values = color
    .match(/[\d.]+/g)
    ?.slice(0, 3)
    .map(Number)
  if (!values || values.length !== 3) throw new Error(`Unsupported computed color: ${color}`)
  return values
}

function luminance(rgb: number[]): number {
  const linear = rgb.map(value => {
    const channel = value / 255
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
}

for (const { theme, button } of [
  { theme: 'dark', button: 'Dark' },
  { theme: 'light', button: 'Light' },
]) {
  for (const viewport of [
    { width: 1200, height: 820 },
    { width: 900, height: 650 },
  ]) {
    test.describe(`Sidebar empty state ${theme} ${viewport.width}`, () => {
      test.use({
        viewport,
      })
      test('keeps informational text readable', async ({ page }, testInfo) => {
        await waitForAppReady(page)
        await page.locator('.activity-bar [aria-label="Settings"]').click()
        await page.locator('.sidebar-panel').getByText('Appearance', { exact: true }).click()
        await page.getByRole('button', { name: button, exact: true }).click()
        await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
        await page.locator('.activity-bar [aria-label="GitHub"]').click()
        const label = page
          .locator('.sidebar-item-empty')
          .getByText('No accounts configured', { exact: true })
        await expect(label).toBeVisible()
        const styles = await label.evaluate(element => {
          const panel = element.closest('.sidebar-panel')
          if (!panel) throw new Error('Empty state has no sidebar panel')
          let opacity = 1
          for (let node: Element | null = element; node; node = node.parentElement) {
            opacity *= Number(getComputedStyle(node).opacity)
          }
          return {
            color: getComputedStyle(element).color,
            background: getComputedStyle(panel).backgroundColor,
            opacity,
          }
        })
        const background = channels(styles.background)
        const foreground = channels(styles.color).map((value, index) =>
          Math.round(value * styles.opacity + background[index] * (1 - styles.opacity))
        )
        const levels = [luminance(foreground), luminance(background)].sort((a, b) => b - a)
        const ratio = (levels[0] + 0.05) / (levels[1] + 0.05)
        expect(ratio).toBeGreaterThanOrEqual(4.5)
        const bounds = await label.boundingBox()
        if (!bounds) throw new Error('Empty state has no rendered bounds')
        expect(bounds.x).toBeGreaterThanOrEqual(0)
        expect(bounds.x + bounds.width).toBeLessThanOrEqual(viewport.width)
        await page.screenshot({
          path: testInfo.outputPath(`sidebar-empty-${theme}-${viewport.width}.png`),
        })
        await testInfo.attach('computed-contrast.json', {
          body: JSON.stringify({ theme, viewport, styles, ratio, bounds }),
          contentType: 'application/json',
        })
      })
    })
  }
}
