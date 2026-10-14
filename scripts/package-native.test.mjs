import assert from 'node:assert/strict'
import { Buffer } from 'node:buffer'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import { URL } from 'node:url'
import { inspectPackage, packageConfiguration, sourceSnapshot } from './package-native.mjs'
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
