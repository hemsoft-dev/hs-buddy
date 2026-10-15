import { test, expect, waitForAppReady } from './fixtures'

test.use({ viewport: { width: 1200, height: 820 }, video: 'on' })

test('reset recovers from secure storage failure and permits a confirmed retry', async ({
  page,
}, testInfo) => {
  await waitForAppReady(page)
  await page.evaluate(() => {
    const original = window.ipcRenderer.invoke.bind(window.ipcRenderer)
    let attempts = 0
    window.ipcRenderer.invoke = async (channel, ...args) => {
      if (channel === 'config:reset') {
        attempts += 1
        return attempts === 1
          ? { success: false, error: 'Disposable secure-store failure fixture' }
          : { success: true }
      }
      return original(channel, ...args)
    }
  })
  await page.locator('.activity-bar [aria-label="Settings"]').click()
  await page.locator('.sidebar-panel').getByText('Advanced', { exact: true }).click()
  await page.getByRole('button', { name: 'Reset to Defaults', exact: true }).click()
  await page.getByRole('button', { name: 'Click Again to Confirm', exact: true }).click()
  await expect(page.getByText('Reset Configuration', { exact: true })).toBeVisible()
  await expect(page.getByRole('alert')).toContainText('Unlock secure storage')
  await expect(page.getByRole('alert')).not.toContainText('Disposable')
  await expect(page.getByRole('button', { name: 'Reset to Defaults', exact: true })).toBeEnabled()
  await page.getByRole('alert').evaluate(element => element.scrollIntoView({ block: 'center' }))
  const message = await page.getByRole('alert').boundingBox()
  const panel = await page.locator('.settings-page:visible').boundingBox()
  if (!message || !panel) throw new Error('Reset failure feedback is not visible')
  expect(message.y).toBeGreaterThanOrEqual(panel.y)
  expect(message.y + message.height).toBeLessThanOrEqual(panel.y + panel.height)
  await page.screenshot({ path: testInfo.outputPath('advanced-secure-reset-error-dark.png') })
  await page.getByRole('button', { name: 'Reset to Defaults', exact: true }).click()
  await page.getByRole('button', { name: 'Click Again to Confirm', exact: true }).click()
  await expect(page.getByRole('alert')).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Reset to Defaults', exact: true })).toBeEnabled()
  await page.screenshot({ path: testInfo.outputPath('advanced-secure-reset-recovered-dark.png') })
})
