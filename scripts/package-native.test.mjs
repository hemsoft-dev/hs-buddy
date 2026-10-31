import assert from 'node:assert/strict'
import { Buffer } from 'node:buffer'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { delimiter, join } from 'node:path'
import process from 'node:process'
import { test } from 'node:test'
import { URL } from 'node:url'
import { inspectPackage, packageConfiguration, sourceSnapshot } from './package-native.mjs'
import { hdiutilPath, runHdiutil } from './package-native-hdiutil.mjs'
import {
  verifyBinaryArchitecture,
  verifyStandaloneMac,
  verifyStandaloneWindows,
} from './native-binary.mjs'

const metadata = JSON.parse(
  readFileSync(new URL('../rust/desktop-package.json', import.meta.url), 'utf8')
)

function directory(t) {
  const root = mkdtempSync(join(tmpdir(), 'buddy native package '))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  return root
}

function diskImageRun(results) {
  const calls = [],
    delays = [],
    output = []
  const invoke = args =>
    runHdiutil(args, {
      run: (...call) => {
        calls.push(call)
        return results[Math.min(calls.length - 1, results.length - 1)]
      },
      pause: ms => delays.push(ms),
      stdout: text => output.push(text),
      stderr: text => output.push(text),
    })
  return { invoke, calls, delays, output }
}

for (const [operation, status, message] of [
  ['create', 1, 'hdiutil: create failed - Resource busy\n'],
  ['detach', 16, 'hdiutil: couldn\'t eject "disk2" - Resource busy\n'],
]) {
  test(`DMG ${operation} retries a busy primitive, retaining arguments, diagnostics and success`, () => {
    const run = diskImageRun([
      { status, stderr: message },
      { status: 0, stdout: 'done\n' },
    ])
    const args = [operation, '/owned build/界 image.dmg']
    assert.equal(run.invoke(args), 0)
    assert.equal(run.calls.length, 2)
    for (const [command, observed] of run.calls) {
      assert.equal(command, '/usr/bin/hdiutil')
      assert.deepEqual(observed, args)
    }
    assert.deepEqual(run.delays, [2000])
    assert.ok(run.output.includes(message))
    assert.ok(run.output.includes('done\n'))
  })
}

test('DMG busy retries are bounded and preserve the final failure', () => {
  const run = diskImageRun([{ status: 16, stderr: 'hdiutil: could not eject - Resource busy\n' }])
  assert.equal(run.invoke(['detach', '/dev/disk2']), 16)
  assert.equal(run.calls.length, 5)
  assert.deepEqual(run.delays, [2000, 4000, 8000, 16000])
})

test('DMG retry consumes real child exit codes and output without changing Unicode arguments', t => {
  const root = directory(t)
  const state = join(root, 'attempt.json')
  const fixture = join(root, 'fake-hdiutil.cjs')
  writeFileSync(
    fixture,
    `
    const fs = require('node:fs');
    const [state, ...args] = process.argv.slice(2);
    const calls = fs.existsSync(state) ? JSON.parse(fs.readFileSync(state)) : [];
    calls.push(args); fs.writeFileSync(state, JSON.stringify(calls));
    if (calls.length === 1) {
      process.stderr.write('hdiutil: create failed - Resource busy\\n');
      process.exitCode = 1;
    } else { process.stdout.write('created image\\n'); }
  `
  )
  const args = ['create', '/owned folder/界 image.dmg']
  const output = []
  const status = runHdiutil(args, {
    run: (_, argv, options) => spawnSync(process.execPath, [fixture, state, ...argv], options),
    pause: () => {},
    stdout: text => output.push(text),
    stderr: text => output.push(text),
  })
  assert.equal(status, 0)
  assert.deepEqual(JSON.parse(readFileSync(state, 'utf8')), [args, args])
  assert.ok(output.includes('hdiutil: create failed - Resource busy\n'))
  assert.ok(output.includes('created image\n'))
})

test('DMG retries never mask other errors, operations, success or termination', () => {
  for (const [operation, result, expected] of [
    ['create', { status: 1, stderr: 'hdiutil: permission denied\n' }, 1],
    ['create', { status: 1, stderr: 'file named Resource busy does not exist\n' }, 1],
    ['create', { status: 16, stderr: 'hdiutil: failed - Resource busy\n' }, 16],
    ['detach', { status: 1, stderr: 'hdiutil: failed - Resource busy\n' }, 1],
    ['attach', { status: 16, stderr: 'hdiutil: failed - Resource busy\n' }, 16],
    ['create', { status: 0, stderr: 'hdiutil: warning - Resource busy\n' }, 0],
    ['detach', { status: null, signal: 'SIGTERM', stderr: 'hdiutil: failed - Resource busy\n' }, 1],
  ]) {
    const run = diskImageRun([result])
    assert.equal(run.invoke([operation]), expected)
    assert.equal(run.calls.length, 1)
    assert.deepEqual(run.delays, [])
  }
  const run = diskImageRun([{ error: new Error('spawn failed') }])
  assert.throws(() => run.invoke(['create']), /spawn failed/)
  assert.equal(run.calls.length, 1)
  assert.deepEqual(run.delays, [])
})

test('DMG shim is private to the package command and preserves argument boundaries', t => {
  const root = directory(t)
  const before = process.env.PATH
  const path = hdiutilPath(root)
  assert.equal(path, `${join(root, '.hdiutil-bin')}${delimiter}${before || ''}`)
  assert.equal(process.env.PATH, before)
  const shim = readFileSync(join(root, '.hdiutil-bin', 'hdiutil'), 'utf8')
  assert.ok(shim.startsWith('#!/bin/sh\nexec '))
  assert.ok(shim.includes('native-hdiutil.mjs'))
  assert.ok(shim.endsWith(' "$@"\n'))
})

function pe(machine = 0x8664) {
  const bytes = Buffer.alloc(128)
  bytes.write('MZ')
  bytes.writeUInt32LE(64, 0x3c)
  bytes.writeUInt32LE(0x4550, 64)
  bytes.writeUInt16LE(machine, 68)
  return bytes
}

test('architecture guard rejects wrong Windows architecture and corrupt PE offsets', () => {
  verifyBinaryArchitecture(pe(), 'x86_64-pc-windows-msvc')
  assert.throws(
    () => verifyBinaryArchitecture(pe(0x14c), 'x86_64-pc-windows-msvc'),
    /does not match/
  )
  const corrupt = pe()
  corrupt.writeUInt32LE(0xffffffff, 0x3c)
  assert.throws(() => verifyBinaryArchitecture(corrupt, 'x86_64-pc-windows-msvc'), /does not match/)
  assert.throws(
    () => verifyBinaryArchitecture(Buffer.alloc(10), 'x86_64-pc-windows-msvc'),
    /truncated/
  )
})

test('architecture guard distinguishes Intel/ARM Mac and 32/64-bit Linux binaries', () => {
  const mach = Buffer.alloc(64)
  mach.writeUInt32LE(0xfeedfacf)
  mach.writeUInt32LE(0x0100000c, 4)
  verifyBinaryArchitecture(mach, 'aarch64-apple-darwin')
  assert.throws(() => verifyBinaryArchitecture(mach, 'x86_64-apple-darwin'), /does not match/)
  const elf = Buffer.alloc(64)
  elf.write('7f454c46', 0, 'hex')
  elf[4] = 2
  elf[5] = 1
  elf.writeUInt16LE(62, 18)
  verifyBinaryArchitecture(elf, 'x86_64-unknown-linux-gnu')
  elf[4] = 1
  assert.throws(() => verifyBinaryArchitecture(elf, 'x86_64-unknown-linux-gnu'), /does not match/)
})

function peImports(names) {
  const bytes = Buffer.alloc(2048)
  pe().copy(bytes)
  bytes.writeUInt16LE(1, 70)
  bytes.writeUInt16LE(240, 84)
  bytes.writeUInt16LE(0x20b, 88)
  bytes.writeUInt32LE(0x1000, 208)
  bytes.writeUInt32LE((names.length + 1) * 20, 212)
  bytes.writeUInt32LE(0x1000, 340)
  bytes.writeUInt32LE(1536, 344)
  bytes.writeUInt32LE(512, 348)
  names.forEach((name, index) => {
    bytes.writeUInt32LE(0x1100 + index * 128, 512 + index * 20 + 12)
    bytes.write(`${name}\0`, 768 + index * 128)
  })
  return bytes
}

test('Windows runtime guard checks imports, rejects runtime DLLs, and ignores unrelated strings', () => {
  const standalone = peImports(['KERNEL32.dll'])
  standalone.write('VCRUNTIME140.dll', 1600)
  assert.deepEqual(verifyStandaloneWindows(standalone), ['kernel32.dll'])
  // System audio APIs ship with Windows; the guard accepts them (rodio 0.23, #884).
  assert.deepEqual(verifyStandaloneWindows(peImports(['WINMM.dll', 'MMDevAPI.dll'])), [
    'winmm.dll',
    'mmdevapi.dll',
  ])
  assert.throws(
    () => verifyStandaloneWindows(peImports(['KERNEL32.dll', 'VCRUNTIME140.dll'])),
    /undeclared runtime DLLs: vcruntime140.dll/
  )
  assert.throws(
    () => verifyStandaloneWindows(peImports(['other-runtime.dll'])),
    /undeclared runtime DLLs/
  )
  const corrupt = peImports(['KERNEL32.dll'])
  corrupt.writeUInt32LE(0xffff0000, 524)
  assert.throws(() => verifyStandaloneWindows(corrupt), /outside file sections/)
})

test('a staged raw binary cannot qualify as a produced installer', t => {
  const root = directory(t)
  mkdirSync(join(root, 'bin'))
  writeFileSync(join(root, 'bin', 'buddy-native.exe'), pe())
  assert.throws(() => inspectPackage(root, 'nsis'), /exactly one/)
  writeFileSync(join(root, 'setup.exe'), Buffer.from('MZ'))
  assert.throws(() => inspectPackage(root, 'nsis'), /Invalid nsis/)
  writeFileSync(join(root, 'setup.exe'), pe(0x14c))
  const artifact = inspectPackage(root, 'nsis')
  assert.equal(artifact.file, 'setup.exe')
  assert.equal(artifact.size, 128)
  writeFileSync(join(root, 'stale-setup.exe'), pe(0x14c))
  assert.throws(() => inspectPackage(root, 'nsis'), /exactly one/)
})

test('Mac runtime guard rejects unbundled Homebrew and rpath libraries', () => {
  const system =
    'buddy:\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1.0.0)'
  assert.deepEqual(verifyStandaloneMac(system), ['/usr/lib/libSystem.B.dylib'])
  for (const library of ['/opt/homebrew/lib/libextra.dylib', '@rpath/libextra.dylib']) {
    assert.throws(
      () =>
        verifyStandaloneMac(
          `${system}\n\t${library} (compatibility version 1.0.0, current version 1.0.0)`
        ),
      /unbundled runtime libraries/
    )
  }
  assert.throws(() => verifyStandaloneMac('buddy:\n unexpected output'), /Could not inspect/)
})

test('Mac bundles the unchanged license notice without an interactive DMG agreement', t => {
  const root = directory(t)
  const options = { metadata, version: '0.1.1333', root, output: root, binaries: root }
  for (const target of ['aarch64-apple-darwin', 'x86_64-apple-darwin']) {
    const config = packageConfiguration({ ...options, target })
    assert.equal(Object.hasOwn(config, 'licenseFile'), false)
    assert.deepEqual(config.resources, [{ src: join(root, 'LICENSE'), target: 'LICENSE' }])
  }
  for (const target of ['x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu']) {
    const config = packageConfiguration({ ...options, target })
    assert.equal(config.licenseFile, join(root, 'LICENSE'))
  }
})

test('package configuration rejects malformed policy values and missing runtime dependencies', t => {
  const root = directory(t)
  const options = {
    metadata,
    version: '0.1.1328',
    target: 'x86_64-unknown-linux-gnu',
    root,
    output: root,
    binaries: root,
  }
  for (const invalid of [null, false, 0, '', []])
    assert.throws(() => packageConfiguration({ ...options, metadata: invalid }), /Invalid desktop/)
  assert.throws(
    () => packageConfiguration({ ...options, metadata: { ...metadata, linuxDependencies: [] } }),
    /runtime dependencies/
  )
  assert.throws(
    () => packageConfiguration({ ...options, metadata: { ...metadata, binaryName: '../other' } }),
    /binary name/
  )
  assert.throws(
    () => packageConfiguration({ ...options, version: 'not-a-version' }),
    /package version/
  )
  const config = packageConfiguration(options)
  assert.equal(config.name, 'buddy-native')
  assert.equal(config.version, '0.1.1328')
  assert.equal(config.binariesDir, root)
  assert.deepEqual(config.deb.depends, metadata.linuxDependencies)
})

test('source snapshot detects changed bytes even while head and dirty flag remain equal', t => {
  const root = directory(t)
  const git = args => execFileSync('git', args, { cwd: root, encoding: 'utf8', windowsHide: true })
  git(['init', '--quiet'])
  writeFileSync(join(root, 'tracked.txt'), 'base\n')
  git(['add', 'tracked.txt'])
  git([
    '-c',
    'user.name=HemSoft',
    '-c',
    'user.email=hemsoft@users.noreply.github.com',
    'commit',
    '--quiet',
    '-m',
    'fixture',
  ])
  writeFileSync(join(root, 'untracked.txt'), 'first\n')
  const before = sourceSnapshot(root)
  writeFileSync(join(root, 'untracked.txt'), 'second\n')
  const after = sourceSnapshot(root)
  assert.equal(before.head, after.head)
  assert.equal(before.dirty, true)
  assert.equal(after.dirty, true)
  assert.notEqual(before.digest, after.digest)
})
