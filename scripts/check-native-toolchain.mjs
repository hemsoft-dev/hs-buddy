import { readFileSync } from 'node:fs'
import console from 'node:console'
import process from 'node:process'
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export function checkNativeToolchain(root = process.cwd()) {
  const manifestPath = resolve(root, 'package.json')
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))
  const pin = manifest.devDependencies?.['node-gyp']
  if (typeof pin !== 'string' || !/^\d+\.\d+\.\d+$/.test(pin)) {
    throw new Error(
      'Declare an exact node-gyp development dependency for native lifecycle scripts.'
    )
  }

  const require = createRequire(manifestPath)
  const installed = require('node-gyp/package.json')
  if (installed.version !== pin) {
    throw new Error(`node-gyp version drift: expected ${pin}, found ${installed.version}.`)
  }

  // Load the same download stack used by rebuild, on the actual Node host.
  // --version alone does not exercise its Node API requirements.
  require('node-gyp/lib/install.js')
  const versionOutput = execFileSync(
    process.execPath,
    [require.resolve('node-gyp/bin/node-gyp.js'), '--version'],
    { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }
  ).trim()
  if (versionOutput !== `v${pin}`) {
    throw new Error(`node-gyp executable drift: expected v${pin}, found ${versionOutput}.`)
  }
  return installed.version
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    console.log(
      `Native build tooling: node-gyp ${checkNativeToolchain()}, Node ${process.versions.node}`
    )
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  }
}
