import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'
import process from 'node:process'
import { fileURLToPath, URL } from 'node:url'
import { test } from 'node:test'
import { checkNativeToolchain } from './check-native-toolchain.mjs'

const script = fileURLToPath(new URL('./check-native-toolchain.mjs', import.meta.url))

function fixture(t, pin, version = '12.4.0', install = 'module.exports = {}') {
  const root = mkdtempSync(join(tmpdir(), 'buddy-native-toolchain-'))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  writeFileSync(
    join(root, 'package.json'),
    JSON.stringify({ devDependencies: { 'node-gyp': pin } })
  )
  const tool = join(root, 'node_modules', 'node-gyp')
  mkdirSync(join(tool, 'lib'), { recursive: true })
  mkdirSync(join(tool, 'bin'))
  writeFileSync(join(tool, 'package.json'), JSON.stringify({ version }))
  writeFileSync(join(tool, 'lib', 'install.js'), install)
  writeFileSync(join(tool, 'bin', 'node-gyp.js'), `console.log('v${version}')`)
  return root
}

test('rejects a missing or floating lifecycle tool through the actual command', t => {
  for (const pin of [undefined, 'latest', '^12.4.0']) {
    const result = spawnSync(process.execPath, [script], { cwd: fixture(t, pin), encoding: 'utf8' })
    assert.equal(result.status, 1)
    assert.match(result.stderr, /Declare an exact node-gyp/)
    assert.equal(result.stdout, '')
  }
})

test('rejects installed version drift instead of qualifying the manifest alone', t => {
  const result = spawnSync(process.execPath, [script], {
    cwd: fixture(t, '12.4.0', '13.1.0'),
    encoding: 'utf8',
  })
  assert.equal(result.status, 1)
  assert.match(result.stderr, /expected 12\.4\.0, found 13\.1\.0/)
})

test('loads download dependencies and preserves their actual startup failure', t => {
  const root = fixture(t, '12.4.0', '12.4.0', "throw new Error('native download API unavailable')")
  assert.throws(() => checkNativeToolchain(root), /native download API unavailable/)
  const result = spawnSync(process.execPath, [script], { cwd: root, encoding: 'utf8' })
  assert.equal(result.status, 1)
  assert.match(result.stderr, /native download API unavailable/)
  assert.equal(result.stdout, '')
})

test('qualifies a matching installed tool and its loadable download stack', t => {
  const result = spawnSync(process.execPath, [script], {
    cwd: fixture(t, '12.4.0'),
    encoding: 'utf8',
  })
  assert.equal(result.status, 0)
  assert.match(result.stdout, /Native build tooling: node-gyp 12\.4\.0, Node/)
  assert.equal(result.stderr, '')
})

test('missing secondary CLI tooling cannot qualify a matching metadata version', t => {
  const root = fixture(t, '12.4.0')
  writeFileSync(
    join(root, 'node_modules', 'node-gyp', 'bin', 'node-gyp.js'),
    "require('buddy-missing-native-cli-dependency')"
  )
  const result = spawnSync(process.execPath, [script], { cwd: root, encoding: 'utf8' })
  assert.equal(result.status, 1)
  assert.match(result.stderr, /Cannot find module 'buddy-missing-native-cli-dependency'/)
  assert.equal(result.stdout, '')
})

test('rejects an executable that disagrees with its installed package metadata', t => {
  const root = fixture(t, '12.4.0')
  writeFileSync(
    join(root, 'node_modules', 'node-gyp', 'bin', 'node-gyp.js'),
    "console.log('v13.1.0')"
  )
  const result = spawnSync(process.execPath, [script], { cwd: root, encoding: 'utf8' })
  assert.equal(result.status, 1)
  assert.match(result.stderr, /executable drift: expected v12\.4\.0, found v13\.1\.0/)
  assert.equal(result.stdout, '')
})
