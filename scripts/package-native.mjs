import { execFileSync } from 'node:child_process'
import console from 'node:console'
import { createHash } from 'node:crypto'
import {
  appendFileSync,
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import {
  verifyBinaryArchitecture,
  verifyStandaloneMac,
  verifyStandaloneWindows,
} from './native-binary.mjs'

const repository = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const targets = {
  'x86_64-pc-windows-msvc': { platform: 'win32', format: 'nsis', extension: '.exe' },
  'x86_64-unknown-linux-gnu': { platform: 'linux', format: 'deb', extension: '' },
  'x86_64-apple-darwin': { platform: 'darwin', format: 'dmg', extension: '' },
  'aarch64-apple-darwin': { platform: 'darwin', format: 'dmg', extension: '' },
}

export function packageConfiguration({ metadata, version, target, root, output, binaries }) {
  if (!metadata || typeof metadata !== 'object' || Array.isArray(metadata))
    throw new Error('Invalid desktop package manifest.')
  for (const key of [
    'productName',
    'identifier',
    'binaryName',
    'packagerVersion',
    'minimumMacOsVersion',
  ]) {
    if (typeof metadata[key] !== 'string' || !metadata[key].trim())
      throw new Error(`Missing desktop package ${key}.`)
  }
  if (!/^[a-z0-9][a-z0-9-]*$/.test(metadata.binaryName))
    throw new Error('Invalid native binary name.')
  if (!/^[A-Za-z0-9]+(?:[.-][A-Za-z0-9]+)+$/.test(metadata.identifier))
    throw new Error('Invalid native application identifier.')
  if (!/^[A-Za-z0-9][A-Za-z0-9 ._-]*$/.test(metadata.productName))
    throw new Error('Invalid native product name.')
  if (
    typeof version !== 'string' ||
    !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)
  )
    throw new Error('Invalid native package version.')
  const options = targets[target]
  if (!options) throw new Error(`Unsupported native package target ${target}.`)
  const config = {
    // cargo-packager 0.11.8 otherwise searches for a name by chdir-ing to
    // the config file itself, which fails before packaging starts.
    name: metadata.binaryName,
    productName: metadata.productName,
    identifier: metadata.identifier,
    version,
    description: 'Your universal productivity companion',
    publisher: 'HemSoft',
    authors: ['HemSoft'],
    homepage: 'https://github.com/hemsoft-dev/hs-buddy',
    category: 'Productivity',
    formats: [options.format],
    outDir: output,
    binariesDir: binaries,
    targetTriple: target,
    binaries: [{ path: metadata.binaryName, main: true }],
    icons: [join(root, 'public', 'icon-512.png'), join(root, 'public', 'icon.ico')],
  }
  // DMG interprets licenseFile as an interactive EULA during mounting. Keep
  // the repository's unchanged notice in the app bundle instead.
  if (options.platform !== 'darwin') config.licenseFile = join(root, 'LICENSE')
  if (options.platform === 'win32') {
    config.nsis = { installMode: 'currentUser', installerIcon: join(root, 'public', 'icon.ico') }
  } else if (options.platform === 'darwin') {
    config.resources = [{ src: join(root, 'LICENSE'), target: 'LICENSE' }]
    config.macos = {
      minimumSystemVersion: metadata.minimumMacOsVersion,
      signingIdentity: '-',
    }
  } else {
    if (
      !Array.isArray(metadata.linuxDependencies) ||
      metadata.linuxDependencies.length === 0 ||
      metadata.linuxDependencies.some(value => typeof value !== 'string' || !value.trim())
    )
      throw new Error('Declare Linux runtime dependencies.')
    config.deb = {
      packageName: metadata.binaryName,
      section: 'utils',
      depends: metadata.linuxDependencies,
      desktopTemplate: join(root, 'rust', 'packaging', 'linux.desktop'),
    }
  }
  return config
}

export function sourceSnapshot(root) {
  const git = args => execFileSync('git', args, { cwd: root, encoding: 'utf8', windowsHide: true })
  const head = git(['rev-parse', 'HEAD']).trim()
  const dirty = git(['status', '--porcelain']).trim().length > 0
  const hash = createHash('sha256').update(git(['diff', '--binary', 'HEAD']))
  for (const path of git(['ls-files', '--others', '--exclude-standard', '-z'])
    .split('\0')
    .filter(Boolean)
    .sort()) {
    hash
      .update(path)
      .update('\0')
      .update(readFileSync(join(root, path)))
      .update('\0')
  }
  return { head, dirty, digest: hash.digest('hex') }
}

export function inspectPackage(output, format) {
  const extension = { nsis: '.exe', deb: '.deb', dmg: '.dmg' }[format]
  if (!extension) throw new Error(`Unsupported package format ${format}.`)
  const files = readdirSync(output, { withFileTypes: true }).filter(
    entry => entry.isFile() && entry.name.endsWith(extension)
  )
  if (files.length !== 1) throw new Error(`Expected exactly one produced ${format} package.`)
  const name = files[0].name
  const bytes = readFileSync(join(output, name))
  const peHeader = bytes.length >= 64 ? bytes.readUInt32LE(0x3c) : bytes.length
  const valid =
    format === 'nsis'
      ? bytes.length >= 64 &&
        bytes.toString('ascii', 0, 2) === 'MZ' &&
        peHeader + 4 <= bytes.length &&
        bytes.readUInt32LE(peHeader) === 0x4550
      : format === 'deb'
        ? bytes.toString('ascii', 0, 8) === '!<arch>\n'
        : bytes.length >= 512 &&
          bytes.toString('ascii', bytes.length - 512, bytes.length - 508) === 'koly'
  if (!valid) throw new Error(`Invalid ${format} package container.`)
  return {
    file: name,
    size: bytes.length,
    sha256: createHash('sha256').update(bytes).digest('hex'),
  }
}

export function packageNative(root = repository, development = false) {
  const run = (executable, args, options = {}) =>
    execFileSync(executable, args, {
      cwd: root,
      encoding: 'utf8',
      windowsHide: true,
      timeout: 60_000,
      ...options,
    })
  const metadata = JSON.parse(readFileSync(join(root, 'rust', 'desktop-package.json'), 'utf8'))
  const { version } = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'))
  const source = sourceSnapshot(root)
  if (!development && source.dirty)
    throw new Error(
      'Release packaging requires a clean committed source tree. Use --dev for local checks.'
    )
  const target = /^host: (.+)$/m.exec(run('rustc', ['-vV']))?.[1]
  const options = targets[target]
  if (!options || options.platform !== process.platform)
    throw new Error(`Build native packages on a supported target host; found ${target}.`)
  const tool =
    process.env.BUDDY_PACKAGER ||
    (process.platform === 'win32' ? 'cargo-packager.exe' : 'cargo-packager')
  const toolVersion = run(tool, ['--version']).trim()
  if (toolVersion !== `cargo-packager ${metadata.packagerVersion}`)
    throw new Error(
      `Install cargo-packager ${metadata.packagerVersion} with --locked; found ${toolVersion}.`
    )
  const outputBase = join(root, 'release', 'native', target, version)
  mkdirSync(outputBase, { recursive: true })
  const output = mkdtempSync(join(outputBase, 'build-'))
  const binaries = join(output, 'bin')
  const config = packageConfiguration({ metadata, version, target, root, output, binaries })
  const args = [
    'build',
    '--locked',
    '--manifest-path',
    join(root, 'rust', 'Cargo.toml'),
    '-p',
    'buddy-app',
    '--message-format=json-render-diagnostics',
  ]
  if (!development) args.push('--release')
  console.log(`Building ${metadata.productName} ${version} (${target})...`)
  const buildOutput = run('cargo', args, {
    cwd: join(root, 'rust'),
    timeout: 60 * 60_000,
    maxBuffer: 32 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'inherit'],
  })
  const artifacts = buildOutput
    .trim()
    .split(/\r?\n/)
    .filter(Boolean)
    .map(line => JSON.parse(line))
  const executables = [
    ...new Set(
      artifacts
        .filter(
          item =>
            item.reason === 'compiler-artifact' && item.target?.name === 'buddy' && item.executable
        )
        .map(item => item.executable)
    ),
  ]
  if (executables.length !== 1)
    throw new Error('Cargo did not report exactly one Buddy executable.')
  const executable = executables[0]
  verifyBinaryArchitecture(readFileSync(executable), target)
  const runtimeLibraries =
    options.platform === 'win32'
      ? verifyStandaloneWindows(readFileSync(executable))
      : options.platform === 'darwin'
        ? verifyStandaloneMac(run('otool', ['-L', executable]))
        : []
  const observedVersion = run(executable, ['--version'], { timeout: 10_000 }).trim()
  if (observedVersion !== `${metadata.productName} ${version}`)
    throw new Error(`Native version mismatch: ${observedVersion}.`)
  mkdirSync(binaries, { recursive: true })
  const stagedBinary = join(binaries, `${metadata.binaryName}${options.extension}`)
  copyFileSync(executable, stagedBinary)
  if (process.platform !== 'win32') chmodSync(stagedBinary, 0o755)
  const configurationPath = join(output, 'packager.json')
  writeFileSync(configurationPath, `${JSON.stringify(config, null, 2)}\n`)
  run(tool, ['--config', configurationPath], { timeout: 30 * 60_000, stdio: 'inherit' })
  const artifact = inspectPackage(output, options.format)
  if (JSON.stringify(sourceSnapshot(root)) !== JSON.stringify(source))
    throw new Error(
      'Source changed while native packaging was running; discard this qualification.'
    )
  const manifest = {
    version,
    target,
    sourceSha: source.head,
    sourceDirty: source.dirty,
    sourceDigest: source.digest,
    profile: development ? 'dev' : 'release',
    packagerVersion: metadata.packagerVersion,
    binarySha256: createHash('sha256').update(readFileSync(stagedBinary)).digest('hex'),
    runtimeLibraries,
    package: artifact,
  }
  writeFileSync(join(output, 'build.json'), `${JSON.stringify(manifest, null, 2)}\n`)
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `directory=${output}\n`)
  console.log(`Native packages: ${output}`)
  return output
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const args = process.argv.slice(2)
    if (args.length > 1 || (args.length === 1 && args[0] !== '--dev'))
      throw new Error('Usage: node scripts/package-native.mjs [--dev]')
    packageNative(repository, args[0] === '--dev')
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  }
}
