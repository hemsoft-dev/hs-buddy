import { test, expect, waitForAppReady } from './fixtures'

test.use({ viewport: { width: 1200, height: 820 }, video: 'on' })

test('clears an unknown credential without requiring a replacement', async ({ page }, testInfo) => {
  const cleared: unknown[] = []
  await page.exposeFunction('buddyRecordCredentialClear', (value: unknown) => {
    cleared.push(value)
  })
  await waitForAppReady(page)
  await page.evaluate(() => {
    const original = window.ipcRenderer.invoke.bind(window.ipcRenderer)
    window.ipcRenderer.invoke = async (channel, ...args) => {
      if (channel === 'config:get-pollen-api-key') {
        return { configured: false, error: 'Disposable migration-conflict fixture' }
      }
      if (channel === 'config:set-pollen-api-key') {
        await (
          window as unknown as {
            buddyRecordCredentialClear: (value: unknown) => Promise<void>
          }
        ).buddyRecordCredentialClear(args[0])
        return { success: true }
      }
      return original(channel, ...args)
    }
  })
  await page.locator('.activity-bar [aria-label="Settings"]').click()
  await page.locator('.sidebar-panel').getByText('Weather', { exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Weather', exact: true })).toBeVisible()
  await expect(page.getByRole('alert')).toContainText('Unlock secure storage')
  await expect(page.getByRole('button', { name: 'Save Key', exact: true })).toBeDisabled()
  await expect(page.getByRole('button', { name: 'Clear Key', exact: true })).toBeEnabled()
  await page.screenshot({ path: testInfo.outputPath('weather-credential-error-dark.png') })
  await page.getByRole('button', { name: 'Clear Key', exact: true }).click()
  await expect(page.getByText('Not configured: add an API key to see pollen data.')).toBeVisible()
  expect(cleared).toEqual([''])
  await expect(page.getByRole('button', { name: 'Clear Key', exact: true })).toHaveCount(0)
  await page.screenshot({ path: testInfo.outputPath('weather-credential-cleared-dark.png') })
})
