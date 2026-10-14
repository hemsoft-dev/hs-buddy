import assert from 'node:assert/strict'
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { delimiter, dirname, join } from 'node:path'
import process from 'node:process'
import { spawnSync } from 'node:child_process'
import { test } from 'node:test'

const require = createRequire(import.meta.url)
const patchedRebuild = join(
  dirname(require.resolve('node-pty/package.json')),
  'scripts',
  'rebuild.js'
)

function fixture(t, status = 0, withTool = true) {
  const root = mkdtempSync(join(tmpdir(), 'buddy native rebuild '))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const pty = join(root, 'node_modules', 'node-pty')
  mkdirSync(join(pty, 'scripts'), { recursive: true })
  const rebuild = join(pty, 'scripts', 'rebuild.js')
  copyFileSync(patchedRebuild, rebuild)
  if (withTool) {
    const bin = join(root, 'node_modules', 'node-gyp', 'bin')
    mkdirSync(bin, { recursive: true })
    writeFileSync(
      join(bin, 'node-gyp.js'),
      `console.log(JSON.stringify({args: process.argv.slice(2), cwd: process.cwd(), executable: process.argv[1]})); process.exit(${status});`
    )
  }
  const shims = join(root, 'shims')
  mkdirSync(shims)
  writeFileSync(join(shims, 'node-gyp.cmd'), '@exit /b 99\r\n')
  writeFileSync(join(shims, 'node-gyp'), '#!/bin/sh\nexit 99\n', { mode: 0o755 })
  return { rebuild, pty, env: { ...process.env, PATH: `${shims}${delimiter}${process.env.PATH}` } }
}

test('native rebuild invokes the installed JS tool despite a hostile PATH shim', t => {
  const target = fixture(t)
  const result = spawnSync(process.execPath, [target.rebuild], {
    cwd: target.pty,
    env: target.env,
    encoding: 'utf8',
  })
  assert.equal(result.status, 0, result.stderr)
  const observed = JSON.parse(result.stdout)
  assert.deepEqual(observed.args, ['rebuild'])
  assert.equal(observed.cwd, target.pty)
  assert.equal(observed.executable, join(dirname(target.pty), 'node-gyp', 'bin', 'node-gyp.js'))
})

test('native rebuild propagates the installed tool failure without retrying the shim', t => {
  const target = fixture(t, 37)
  const result = spawnSync(process.execPath, [target.rebuild], {
    cwd: target.pty,
    env: target.env,
    encoding: 'utf8',
  })
  assert.equal(result.status, 37)
  assert.equal(result.stdout.trim().split('\n').length, 1)
})

test('missing installed tool fails without downloading a floating replacement', t => {
  const target = fixture(t, 0, false)
  const result = spawnSync(process.execPath, [target.rebuild], {
    cwd: target.pty,
    env: target.env,
    encoding: 'utf8',
  })
  assert.equal(result.status, 1)
  assert.match(result.stderr, /Cannot find module 'node-gyp\/bin\/node-gyp\.js'/)
  assert.equal(result.stdout, '')
})
