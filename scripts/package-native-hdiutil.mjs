import { spawnSync } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'
import { delimiter, join, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const script = fileURLToPath(import.meta.url)
const attempts = 5
const sleep = ms => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms)

// Keep the retry at the disk-image primitive: restarting cargo-packager could
// leave a mounted partial image behind. Never force-eject or remove a volume.
export function runHdiutil(
  args,
  {
    run = spawnSync,
    pause = sleep,
    stdout = text => process.stdout.write(text),
    stderr = text => process.stderr.write(text),
  } = {}
) {
  const operation = args[0]
  const retryOperation = operation === 'create' || operation === 'detach'
  for (let attempt = 1; attempt <= attempts; attempt++) {
    const result = run('/usr/bin/hdiutil', args, {
      encoding: 'utf8',
      timeout: 180_000,
      maxBuffer: 4 * 1024 * 1024,
      windowsHide: true,
      stdio: retryOperation ? 'pipe' : 'inherit',
    })
    if (result.stdout) stdout(result.stdout)
    if (result.stderr) stderr(result.stderr)
    if (result.error) throw result.error
    if (result.signal || !Number.isInteger(result.status)) return 1
    const busy =
      retryOperation &&
      result.status === (operation === 'create' ? 1 : 16) &&
      /^hdiutil:.* - Resource busy\s*$/m.test(`${result.stdout || ''}\n${result.stderr || ''}`)
    if (!busy || attempt === attempts) return result.status
    const delay = 1000 * 2 ** attempt
    stderr(`hdiutil ${operation} is busy; retry ${attempt + 1}/${attempts} in ${delay / 1000}s\n`)
    pause(delay)
  }
}

// Only cargo-packager and its children get this private PATH. The shim uses an
// absolute Node path and the runner's absolute hdiutil, avoiding PATH recursion.
export function hdiutilPath(output) {
  const directory = join(output, '.hdiutil-bin')
  mkdirSync(directory, { mode: 0o700 })
  const quote = value => `'${value.replaceAll("'", "'\"'\"'")}'`
  writeFileSync(
    join(directory, 'hdiutil'),
    `#!/bin/sh\nexec ${quote(process.execPath)} ${quote(script)} "$@"\n`,
    { mode: 0o700 }
  )
  return `${directory}${delimiter}${process.env.PATH || ''}`
}

if (process.argv[1] && resolve(process.argv[1]) === script) {
  try {
    process.exitCode = runHdiutil(process.argv.slice(2))
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`)
    process.exitCode = 1
  }
}
