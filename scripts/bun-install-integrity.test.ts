// @vitest-environment node
import { spawn, type ChildProcess } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdtemp, realpath, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { basename, dirname, join } from 'node:path'
import { gzipSync } from 'node:zlib'
import { expect, it } from 'vitest'

type InstallResources = { child?: ChildProcess }
type InstallResult = { code: number | null; timedOut: boolean; stderr: string }

function runInstall(directory: string, resources: InstallResources): Promise<InstallResult> {
  const environment: NodeJS.ProcessEnv = {
    PATH: process.env.PATH,
    SYSTEMROOT: process.env.SYSTEMROOT,
    WINDIR: process.env.WINDIR,
    HOME: directory,
    USERPROFILE: directory,
    TEMP: directory,
    TMP: directory,
    BUN_INSTALL_CACHE_DIR: join(directory, 'cache'),
    NO_COLOR: '1',
  }
  const child = spawn(
    process.env.BUDDY_INSTALLER_TEST_BUN ?? 'bun',
    ['install', '--ignore-scripts', '--no-cache'],
    {
      cwd: directory,
      env: environment,
      windowsHide: true,
      stdio: ['ignore', 'pipe', 'pipe'],
    }
  )
  resources.child = child
  return new Promise((resolve, reject) => {
    let stderr = ''
    let timedOut = false
    const deadline = setTimeout(() => {
      timedOut = true
      child.kill('SIGKILL')
    }, 5000)
    child.stdout?.resume()
    child.stderr?.on('data', chunk => {
      stderr += String(chunk)
    })
    child.once('error', error => {
      clearTimeout(deadline)
      reject(error)
    })
    child.once('close', code => {
      clearTimeout(deadline)
      resolve({ code, timedOut, stderr })
    })
  })
}

it('rejects a corrupt isolated-install tarball promptly while enforcing integrity', async ({
  onTestFinished,
}) => {
  const temporaryRoot = await realpath(tmpdir())
  const directory = await mkdtemp(join(temporaryRoot, 'buddy-bun-integrity-'))
  const tarball = gzipSync(Buffer.alloc(1024))
  const integrity = `sha512-${createHash('sha512').update('different content').digest('base64')}`
  const requests: string[] = []
  let origin = ''
  const resources: InstallResources = {}
  const server = createServer((request, response) => {
    requests.push(request.url ?? '')
    const metadata = JSON.stringify({
      name: 'fixture-integrity',
      'dist-tags': { latest: '1.0.0' },
      versions: {
        '1.0.0': {
          name: 'fixture-integrity',
          version: '1.0.0',
          dist: { tarball: `${origin}/fixture.tgz`, integrity },
        },
      },
    })
    const body = request.url === '/fixture-integrity' ? Buffer.from(metadata) : tarball
    response.writeHead(200, { 'content-length': body.length })
    response.end(body)
  })
  onTestFinished(async () => {
    const child = resources.child
    if (child?.exitCode === null && child.signalCode === null) child.kill('SIGKILL')
    server.closeAllConnections()
    await new Promise<void>(resolve => server.close(() => resolve()))
    const owned = await realpath(directory)
    if (dirname(owned) !== temporaryRoot || !basename(owned).startsWith('buddy-bun-integrity-')) {
      throw new Error('Preserve an unexpected installer fixture path')
    }
    await rm(owned, { recursive: true, force: true })
  })
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve))
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('Missing owned registry address')
  origin = `http://127.0.0.1:${address.port}`
  await writeFile(
    join(directory, 'package.json'),
    JSON.stringify({
      name: 'owned-install-fixture',
      dependencies: { 'fixture-integrity': '1.0.0' },
    })
  )
  await writeFile(
    join(directory, 'bunfig.toml'),
    `[install]\nlinker = "isolated"\nregistry = "${origin}"\n`
  )
  const result = await runInstall(directory, resources)
  expect(requests).toContain('/fixture-integrity')
  expect(requests).toContain('/fixture.tgz')
  expect(result.stderr).toContain('Integrity check failed')
  expect(result.timedOut, result.stderr).toBe(false)
  expect(result.code).not.toBe(0)
  expect(result.code).not.toBeNull()
}, 15000)
