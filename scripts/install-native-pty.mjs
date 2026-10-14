import { spawnSync } from 'node:child_process'
import console from 'node:console'
import { createRequire } from 'node:module'
import { dirname, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { checkNativeToolchain } from './check-native-toolchain.mjs'

// Bun's isolated linker can start dependency lifecycles before a sibling's
// transitive links exist. Run node-pty's lifecycle from the root postinstall.
export function installNativePty(root = process.cwd()) {
  const require = createRequire(resolve(root, 'package.json'))
  const ptyRoot = dirname(require.resolve('node-pty/package.json'))
  function run(script, args = []) {
    const result = spawnSync(process.execPath, [script, ...args], {
      cwd: ptyRoot,
      stdio: 'inherit',
    })
    if (result.error) throw result.error
    if (result.signal) throw new Error(`Native install terminated by ${result.signal}.`)
    return result.status ?? 1
  }

  const prebuildStatus = run(resolve(ptyRoot, 'scripts/prebuild.js'))
  if (prebuildStatus !== 0) {
    if (prebuildStatus !== 1) return prebuildStatus
    checkNativeToolchain(root)
    // Resolve the installed binary directly, bypassing Bun's @latest PATH shim.
    const buildStatus = run(require.resolve('node-gyp/bin/node-gyp.js'), ['rebuild'])
    if (buildStatus !== 0) return buildStatus
  }
  return run(resolve(ptyRoot, 'scripts/post-install.js'))
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = installNativePty()
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  }
}
