import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { delimiter, join } from 'node:path'
import process from 'node:process'
import { test } from 'node:test'
import { fileURLToPath, URL } from 'node:url'

const installer = fileURLToPath(new URL('./install-native-pty.mjs', import.meta.url))

function fixture(t, { prebuild = 1, build = 0, cleanup = 0, missingSecondary = false } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'buddy native install '))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  writeFileSync(
    join(root, 'package.json'),
    JSON.stringify({ devDependencies: { 'node-gyp': '12.4.0' } })
  )
  const pty = join(root, 'node_modules', 'node-pty')
  const gyp = join(root, 'node_modules', 'node-gyp')
  mkdirSync(join(pty, 'scripts'), { recursive: true })
  mkdirSync(join(gyp, 'bin'), { recursive: true })
  mkdirSync(join(gyp, 'lib'), { recursive: true })
  writeFileSync(join(pty, 'package.json'), '{}')
  writeFileSync(join(gyp, 'package.json'), '{"version":"12.4.0"}')
  writeFileSync(
    join(gyp, 'lib', 'install.js'),
    missingSecondary ? "require('./missing-download-dependency')" : ''
  )
  const log = join(root, 'calls.jsonl')
  const record = stage =>
    `require('node:fs').appendFileSync(${JSON.stringify(log)}, JSON.stringify({stage:${JSON.stringify(stage)},args:process.argv.slice(2),cwd:process.cwd(),executable:process.argv[1]})+'\\n');`
  writeFileSync(
    join(pty, 'scripts', 'prebuild.js'),
    `${record('prebuild')} process.exit(${prebuild});`
  )
  writeFileSync(
    join(pty, 'scripts', 'post-install.js'),
    `${record('cleanup')} process.exit(${cleanup});`
  )
  writeFileSync(
    join(gyp, 'bin', 'node-gyp.js'),
    `if(process.argv[2]==='--version'){console.log('v12.4.0')}else{${record('build')}process.exit(${build})}`
  )
  const shims = join(root, 'shims')
  mkdirSync(shims)
  writeFileSync(join(shims, 'node-gyp.cmd'), '@exit /b 99\r\n')
  writeFileSync(join(shims, 'node-gyp'), '#!/bin/sh\nexit 99\n', { mode: 0o755 })
  return {
    root,
    pty,
    gyp,
    log,
    env: { ...process.env, PATH: `${shims}${delimiter}${process.env.PATH}` },
  }
}

function exercise(target) {
  const result = spawnSync(process.execPath, [installer], {
    cwd: target.root,
    env: target.env,
    encoding: 'utf8',
  })
  const calls = readFileSync(target.log, 'utf8')
    .trim()
    .split('\n')
    .map(line => JSON.parse(line))
  return { result, calls }
}

test('root postinstall builds with installed tooling, then runs upstream cleanup', t => {
  const target = fixture(t)
  const { result, calls } = exercise(target)
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(
    calls.map(call => call.stage),
    ['prebuild', 'build', 'cleanup']
  )
  assert.deepEqual(calls[1].args, ['rebuild'])
  assert.equal(calls[1].cwd, target.pty)
  assert.equal(calls[1].executable, join(target.gyp, 'bin', 'node-gyp.js'))
})

test('available prebuild skips build tooling and retains upstream cleanup', t => {
  const target = fixture(t, { prebuild: 0 })
  rmSync(target.gyp, { recursive: true })
  const { result, calls } = exercise(target)
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(
    calls.map(call => call.stage),
    ['prebuild', 'cleanup']
  )
})

test('build failure propagates exactly without retry or cleanup', t => {
  const { result, calls } = exercise(fixture(t, { build: 37 }))
  assert.equal(result.status, 37)
  assert.deepEqual(
    calls.map(call => call.stage),
    ['prebuild', 'build']
  )
})

test('missing secondary tool dependency fails before building or cleanup', t => {
  const { result, calls } = exercise(fixture(t, { missingSecondary: true }))
  assert.equal(result.status, 1)
  assert.match(result.stderr, /missing-download-dependency/)
  assert.deepEqual(
    calls.map(call => call.stage),
    ['prebuild']
  )
})

test('unexpected prebuild failure is preserved without a build attempt', t => {
  const { result, calls } = exercise(fixture(t, { prebuild: 42 }))
  assert.equal(result.status, 42)
  assert.deepEqual(
    calls.map(call => call.stage),
    ['prebuild']
  )
})

test('upstream cleanup failure prevents installation from qualifying', t => {
  const { result, calls } = exercise(fixture(t, { cleanup: 29 }))
  assert.equal(result.status, 29)
  assert.deepEqual(
    calls.map(call => call.stage),
    ['prebuild', 'build', 'cleanup']
  )
})
