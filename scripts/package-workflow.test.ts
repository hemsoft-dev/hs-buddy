import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

const workflow = readFileSync('.github/workflows/ci.yml', 'utf8').replaceAll('\r\n', '\n')
const builderConfig = readFileSync('electron-builder.json5', 'utf8')
const packageJson = readFileSync('package.json', 'utf8')
const smokeRunner = readFileSync('scripts/package-smoke.ts', 'utf8')
const mainProcess = readFileSync('electron/main.ts', 'utf8')
const runtimeQualification = readFileSync('electron/packageQualification.ts', 'utf8')
const windowsInstaller = readFileSync('scripts/install-windows-package.ps1', 'utf8')
const afterPack = readFileSync('scripts/after-pack.mjs', 'utf8')

function parseNeeds(job: string | undefined): string[] {
  if (!job) throw new Error('Missing ci-complete job')
  const declaration = job.match(/^ {4}needs:\s*(?:\[([^\]]+)\]|((?:\n {6}- [^\n]+)+))/m)
  if (!declaration) throw new Error('ci-complete needs must be an inline or multiline array')
  if (declaration[1]) return declaration[1].split(',').map(value => value.trim())
  return [...(declaration[2] ?? '').matchAll(/^\s*-\s*(\S+)/gm)].map(match => match[1])
}

const qualifiedTargets = [
  {
    name: 'Windows x64',
    runner: 'windows-2025',
    builderPlatform: 'win',
    target: 'nsis',
    platform: 'win32',
    arch: 'x64',
  },
  {
    name: 'Linux x64',
    runner: 'ubuntu-24.04',
    builderPlatform: 'linux',
    target: 'deb',
    platform: 'linux',
    arch: 'x64',
  },
  {
    name: 'macOS Intel',
    runner: 'macos-15-intel',
    builderPlatform: 'mac',
    target: 'dmg',
    platform: 'darwin',
    arch: 'x64',
  },
  {
    name: 'macOS Apple silicon',
    runner: 'macos-15',
    builderPlatform: 'mac',
    target: 'dmg',
    platform: 'darwin',
    arch: 'arm64',
  },
] as const

it('qualifies completion audio tests at the advertised Node floor', () => {
  const testJob = workflow.split('  test:')[1]?.split(/\n {2}[\w-]+:/, 1)[0]
  expect(testJob).toMatch(
    /node-version: 22\.12\.0[\s\S]*run: bun run test:completion-audio[\s\S]*node-version: 24\.12\.0/
  )
  expect(JSON.parse(packageJson).scripts['test:completion-audio']).toBe(
    'node --experimental-strip-types --test .pi/tests/done-sound.test.mjs'
  )
})

it('bounds the optional package-smoke dependency cache restore', () => {
  const packageJob = workflow.split('  package-smoke:')[1]?.split(/\n {2}[\w-]+:/, 1)[0]

  expect(packageJob).toMatch(
    /- name: Cache Bun dependencies\n\s+timeout-minutes: 5\n\s+continue-on-error: true\n\s+uses: actions\/cache@/
  )
})

it('keeps Spotlight off the native DMG while it is detached', () => {
  // Native macOS packaging failed twice (main 201a, #839 c14ca7c7) when
  // create-dmg could not detach its volume: "hdiutil: couldn't eject - Resource busy".
  const native = readFileSync('.github/workflows/rust.yml', 'utf8').replaceAll('\r\n', '\n')
  const indexing = native.indexOf('sudo mdutil -a -i off')
  const packaging = native.indexOf('- name: Build native release package')
  expect(indexing).toBeGreaterThan(-1)
  expect(indexing).toBeLessThan(packaging)
  const step = native.slice(native.lastIndexOf('- name:', indexing), indexing)
  expect(step).toContain("if: runner.os == 'macOS'")
})

it('gives native jobs measured headroom with a bounded hang limit', () => {
  // A healthy Intel run on main hit the old 150-minute limit (#852).
  const native = readFileSync('.github/workflows/rust.yml', 'utf8')
  const limit = native.match(
    /timeout-minutes: \$\{\{ matrix\.os == 'macos-15-intel' && (\d+) \|\| (\d+) \}\}/
  )
  expect(Number(limit?.[1])).toBeGreaterThanOrEqual(210)
  expect(Number(limit?.[2])).toBeGreaterThanOrEqual(120)
  expect(Number(limit?.[1])).toBeLessThanOrEqual(240)
})

it('fails the Windows OCR install when Chocolatey installs nothing', () => {
  // A 503 from the Chocolatey feed printed "installed 0/0 packages", the step
  // passed, and the job failed about 50 minutes later at tesseract (#851).
  const native = readFileSync('.github/workflows/rust.yml', 'utf8').replaceAll('\r\n', '\n')
  const step = native.split('- name: Install Windows OCR')[1]?.split('\n      - name:', 1)[0]

  expect(step).toContain("if: runner.os == 'Windows'")
  expect(step).toContain('$attempts = 3')
  expect(step).toContain('Start-Sleep -Seconds 30')
  expect(step).toContain("$tools = [ordered]@{ tesseract = '--version'; ffmpeg = '-version' }")
  expect(step).toContain('Get-Command $_ -ErrorAction SilentlyContinue')
  expect(step).toContain('& $tool $tools[$tool]')
  expect(step).toContain('if ($LASTEXITCODE -ne 0) { throw "$tool')
  const pathPrepend = step?.indexOf('$env:PATH = "$tesseractDirectory;$env:PATH"') ?? -1
  expect(pathPrepend).toBeGreaterThan(-1)
  expect(pathPrepend).toBeLessThan(step?.indexOf('choco install') ?? -1)
})

describe('desktop package qualification workflow', () => {
  it.each(qualifiedTargets)('builds and starts $name packages on a native runner', target => {
    const matrixEntry = workflow.split(`- name: ${target.name}`)[1]?.split(/\n\s+- name: /, 1)[0]

    expect(matrixEntry).toBeDefined()
    for (const setting of [
      `runner: ${target.runner}`,
      `builder-platform: ${target.builderPlatform}`,
      `target: ${target.target}`,
      `platform: ${target.platform}`,
      `arch: ${target.arch}`,
    ]) {
      expect(matrixEntry).toContain(setting)
    }
    expect(workflow).toContain(
      'bunx electron-builder --${{ matrix.builder-platform }} ${{ matrix.target }} --${{ matrix.arch }} --publish never --config.npmRebuild=false'
    )
    expect(workflow).toContain('bun run package:smoke -- ${{ matrix.platform }} ${{ matrix.arch }}')
  })

  it('packages Linux at the advertised minimum Node version', () => {
    expect(JSON.parse(packageJson).engines.node).toBe('>=22.12.0')
    const linuxEntry = workflow.split('- name: Linux x64')[1]?.split(/\n\s+- name: /, 1)[0]
    expect(linuxEntry).toContain('node-version: 22.12.0')
    expect(workflow).toContain("node-version: ${{ matrix.node-version || '24.12.0' }}")
  })

  it('launches each generated distributable rather than its staging tree', () => {
    expect(workflow).toContain('sudo apt-get install --yes "./$package_file"')
    expect(workflow).toContain('./scripts/install-windows-package.ps1')
    expect(windowsInstaller).toContain('Get-ChildItem $ReleaseDirectory -Recurse')
    expect(windowsInstaller).toContain("Start-Process $installer -ArgumentList @('/S'")
    expect(windowsInstaller).toContain('BUDDY_PACKAGE_EXECUTABLE=$executable')
    expect(workflow).toContain('hdiutil attach "$dmg"')
    expect(workflow.match(/BUDDY_PACKAGE_EXECUTABLE=/g)).toHaveLength(2)
    expect(packageJson).toContain('"package:smoke": "bun scripts/package-smoke.ts"')
    expect(smokeRunner).toContain('process.env.BUDDY_PACKAGE_EXECUTABLE')
  })

  it('qualifies packages before CI can complete', () => {
    const ciComplete = workflow.split('  ci-complete:')[1]?.split(/\n {2}[\w-]+:/, 1)[0]

    expect(parseNeeds(ciComplete)).toContain('package-smoke')
    expect(workflow).toContain('if-no-files-found: warn')
    expect(workflow).toContain('release/package-smoke*.log')
    expect(workflow).toMatch(/name: Upload failed distributable\n\s+if: failure\(\)/)
  })

  it('keeps the advertised builder targets aligned with the matrix', () => {
    for (const value of ['nsis', 'deb', 'dmg', 'arm64', 'x64']) {
      expect(builderConfig).toMatch(new RegExp(`["']${value}["']`))
    }
    expect(builderConfig).toContain("maintainer: 'HemSoft <hemsoft@users.noreply.github.com>'")
    expect(builderConfig).toContain("afterPack: 'scripts/after-pack.mjs'")
    expect(afterPack).toContain("'app.asar.unpacked'")
    expect(afterPack).toContain("'node-pty'")
    expect(afterPack).toContain('readdir(nodePtyDirectory, { recursive: true })')
    expect(afterPack).toContain("endsWith('/spawn-helper')")
    expect(afterPack).toContain('0o755')
    expect(afterPack).toContain("context.electronPlatformName === 'win32'")
    expect(afterPack).toContain('await access(helper, constants.X_OK)')
    expect(afterPack).toContain('throw new Error(')
    expect(afterPack).toContain('${context.appOutDir}')
  })

  it('checks the renderer and native dependencies from the packaged runtime', () => {
    expect(mainProcess).toContain('window.__buddyPreloadReady === true')
    expect(mainProcess).toContain('document.getElementById("root")?.childElementCount > 0')
    expect(mainProcess).toContain('qualifyPackageDependencies(')
    expect(mainProcess).toContain('waitForMountedRenderer(')
    expect(mainProcess).toContain('process.resourcesPath')
    for (const dependency of ['node-pty', 'koffi']) {
      expect(runtimeQualification).toContain(`'${dependency}'`)
      expect(smokeRunner).toContain(`'${dependency}'`)
    }
    expect(runtimeQualification).toContain('@github/copilot-${platform}-${arch}')
    expect(smokeRunner).toContain('copilot-${target.platform}-${target.arch}')
    expect(smokeRunner).not.toContain("'--no-sandbox'")
    expect(smokeRunner).toContain("process.kill(-child.pid, 'SIGKILL')")
    expect(smokeRunner).toContain("spawnSync('taskkill'")
    expect(smokeRunner).toContain("detached: platform !== 'win32'")
    expect(smokeRunner).toContain("child.once('exit'")
    expect(smokeRunner).toContain("child.once('close'")
    expect(smokeRunner).toContain('5_000')
  })
})
