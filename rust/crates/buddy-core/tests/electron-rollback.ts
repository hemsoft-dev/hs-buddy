// Run with Bun after `bun install --frozen-lockfile` and building the probe.
// Conf is electron-store's actual persistence/validation implementation.
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { configSchema, defaultConfig } from '../../../../src/types/config'

const require = createRequire(import.meta.url)
const storeRequire = createRequire(require.resolve('electron-store'))
const { default: Conf } = await import(storeRequire.resolve('conf'))
const directory = mkdtempSync(join(tmpdir(), 'buddy-rollback-'))
try {
  const electron = new Conf({ cwd: directory, schema: configSchema, defaults: defaultConfig })
  const fixture = JSON.parse(
    readFileSync(new URL('./fixtures/electron-config.json', import.meta.url), 'utf8')
  )
  electron.set(fixture)
  const before = electron.store
  const probe = process.argv[2]
  assert.ok(probe, 'Pass the built config-migration-probe executable')
  const result = spawnSync(probe, { input: JSON.stringify(before), encoding: 'utf8' })
  assert.equal(result.status, 0, result.stderr)
  const native = JSON.parse(result.stdout)
  // Validate and write through the same Conf schema used by Electron.
  electron.store = native
  const restarted = new Conf({ cwd: directory, schema: configSchema, defaults: defaultConfig })
  // Conf returns a null-prototype root; compare persisted JSON semantics.
  assert.deepEqual(structuredClone(restarted.store), native)
  const expected = structuredClone(before)
  expected.ui.dashboardCards.weather = false
  for (const key of Object.keys(expected)) assert.deepEqual(restarted.get(key), expected[key])
  assert.equal(restarted.get('native.activeSection'), 'settings')
  // Show the contract actually rejects an incompatible known-field change.
  assert.throws(() => {
    electron.store = { ...native, ui: { ...native.ui, theme: 123 } }
  })
  console.log(
    'PASS: Electron schema, unknown keys, ciphertext, native additions, restart, invalid-schema control'
  )
} finally {
  rmSync(directory, { recursive: true, force: true })
}
