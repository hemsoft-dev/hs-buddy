import { readdirSync, readFileSync } from 'node:fs'
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

it('runs the installed macOS settings scene only on request until #913 is fixed', () => {
  // Both Macs failed the settings scene on every #902 head while main passed.
  // The scene stays intact behind a dispatch input; other platforms still run it.
  const native = readFileSync('.github/workflows/rust.yml', 'utf8').replaceAll('\r\n', '\n')
  const unix = readFileSync('scripts/native-smoke/install-unix.sh', 'utf8').replaceAll('\r\n', '\n')
  expect(native).toContain('  workflow_dispatch:\n    inputs:\n      mac_settings_qualification:\n')
  const install = native
    .split('- name: Install and qualify Unix package')[1]
    ?.split('\n      - name:', 1)[0]
  expect(install).toContain(
    "BUDDY_MAC_SETTINGS_QUALIFICATION: ${{ inputs.mac_settings_qualification && '1' || '0' }}"
  )
  const darwin = unix.indexOf('if [[ "$(uname -s)" == Darwin ]]; then')
  const mac = unix.slice(darwin, unix.indexOf('\nelse\n', darwin))
  const gate = mac.indexOf('if [[ "${BUDDY_MAC_SETTINGS_QUALIFICATION:-0}" != 1 ]]; then')
  expect(gate).toBeGreaterThan(-1)
  const scenes = [
    ...mac.matchAll(/scene settings python3 "\$script_directory\/settings-desktop\.py"/g),
  ]
  expect(scenes).toHaveLength(2)
  for (const scene of scenes) expect(scene.index).toBeGreaterThan(gate)
  expect(mac).toContain('scene github python3')
  expect(unix.slice(0, darwin)).toContain('python3 "$1/settings-desktop.py"')
  expect(readFileSync('scripts/native-smoke/install-windows.ps1', 'utf8')).toContain(
    "'settings-desktop.py'"
  )
})

it('gives native jobs measured headroom with a bounded hang limit', () => {
  // #852 exceeded 150m on Intel. #899 ARM spent 51m building/testing and
  // 69m qualifying before cancellation; #901's completed qualification took 64m.
  const native = readFileSync('.github/workflows/rust.yml', 'utf8')
  const limit = native.match(
    /^ {4}timeout-minutes: \$\{\{ matrix\.os == 'macos-15-intel' && (\d+) \|\| matrix\.os == 'macos-15' && (\d+) \|\| (\d+) \}\}/m
  )
  const [intel, arm, other] = limit?.slice(1).map(Number) ?? []
  expect(intel).toBeGreaterThanOrEqual(210)
  expect(intel).toBeLessThanOrEqual(240)
  expect(arm).toBeGreaterThanOrEqual(150)
  expect(arm).toBeLessThanOrEqual(180)
  expect(other).toBe(120)
  const install = native
    .split('- name: Install and qualify Unix package')[1]
    ?.split('\n      - name:', 1)[0]
  const budget = install?.match(
    /timeout-minutes: \$\{\{ matrix\.os == 'macos-15-intel' && (\d+) \|\| (\d+) \}\}/
  )
  // A completed 64m suite fits, and a 51m build plus the whole scene budget
  // still leaves time for evidence staging/upload before the job's hard kill.
  const [intelSuite, armSuite] = budget?.slice(1).map(Number) ?? []
  expect(armSuite).toBeGreaterThan(64)
  expect(51 + armSuite + 5).toBeLessThan(arm)
  expect(intelSuite + 5).toBeLessThan(intel)
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
    expect(workflow).toContain('buddy_apt sudo install --yes "./$package_file"')
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

describe('bounded apt installs', () => {
  const helperPath = 'scripts/lib/bounded-apt.sh'
  const read = (path: string) => readFileSync(path, 'utf8').replaceAll('\r\n', '\n')
  const helper = read(helperPath)
  const workflowPaths = readdirSync('.github/workflows')
    .filter(name => name.endsWith('.yml') && !name.endsWith('.lock.yml'))
    .map(name => `.github/workflows/${name}`)
  const scriptPaths = readdirSync('scripts', { recursive: true, encoding: 'utf8' })
    .filter(name => name.endsWith('.sh'))
    .map(name => `scripts/${name.replaceAll('\\', '/')}`)

  it('sends every apt update and install through the bounded helper', () => {
    const bareApt = /\bapt(?:-get)?\s+(?:-\S+\s+)*(?:update|install|upgrade|dist-upgrade)\b/
    const offenders = [...workflowPaths, ...scriptPaths]
      .filter(path => path !== helperPath)
      .flatMap(path =>
        read(path)
          .split('\n')
          .map((line, index) => ({ line, location: `${path}:${index + 1}` }))
          .filter(({ line }) => !line.trimStart().startsWith('#') && bareApt.test(line))
          .map(({ location, line }) => `${location}: ${line.trim()}`)
      )
    expect(offenders).toEqual([])
    expect(scriptPaths).toContain(helperPath)
  })

  it('bounds each attempt and names the mirror and step on failure', () => {
    expect(helper).toContain('timeout --kill-after=30 "$limit" apt-get')
    expect(helper).toContain('timeout --kill-after=30 "$limit" dpkg --configure -a')
    expect(helper).toContain('limit="${BUDDY_APT_LIMIT:-$limit}"')
    expect(helper).toContain(
      '-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30'
    )
    expect(helper).toContain('local limit=600')
    expect(helper).toContain('if [[ "$step" == update ]]; then limit=300; fi')
    expect(helper).toContain('Step: apt-get %s. Mirrors: %s')
  })

  it('limits every workflow step that installs packages', () => {
    const steps = ['.github/workflows/ci.yml', '.github/workflows/rust.yml'].flatMap(path =>
      read(path)
        .split(/\n\s+- (?=name:|uses:)/)
        .filter(step => step.includes('buddy_apt'))
    )
    expect(steps).toHaveLength(5)
    for (const step of steps) {
      expect(step).toMatch(/\n\s+timeout-minutes: \d+\n/)
      expect(step).toContain('source scripts/lib/bounded-apt.sh')
    }
  })

  it('gives every apt step and its job room for both attempts and the failure message', () => {
    // Per buddy_apt call: two attempts, each allowed 30 s to die after its
    // limit; an install also runs one bounded dpkg recovery.
    const worstSeconds = (step: string) => {
      const override = step.match(/BUDDY_APT_LIMIT: '(\d+)'/)
      return [...step.matchAll(/buddy_apt sudo (update|install)/g)].reduce((total, [, kind]) => {
        const limit = override ? Number(override[1]) : kind === 'update' ? 300 : 600
        return total + (kind === 'update' ? 2 : 3) * (limit + 30)
      }, 0)
    }
    let checked = 0
    for (const path of ['.github/workflows/ci.yml', '.github/workflows/rust.yml']) {
      for (const job of read(path).split(/\n {2}(?=[\w-]+:\n)/)) {
        const jobLimit = job.match(/\n {4}timeout-minutes: (\d+)\n/)
        for (const step of job
          .split(/\n\s+- (?=name:|uses:)/)
          .filter(s => s.includes('buddy_apt'))) {
          const stepLimit = Number(step.match(/\n\s+timeout-minutes: (\d+)\n/)?.[1])
          expect(stepLimit * 60, step).toBeGreaterThanOrEqual(worstSeconds(step))
          if (jobLimit) expect(Number(jobLimit[1]), step).toBeGreaterThanOrEqual(stepLimit + 5)
          checked += 1
        }
      }
    }
    expect(checked).toBe(5)
  })
})

describe('performance runtime provisioning', () => {
  // The performance job runs the prebuilt native binary and an unpacked
  // Electron build; it compiles nothing. Run 42134408589 timed out on Ubuntu
  // fetching 181 packages (96.9 MB) for libwebkit2gtk-4.1-dev and libgtk-3-dev.
  // Under the same signed noble index, 101 of them (13.7 MB) were headers,
  // introspection data and tools pulled in only by that development closure.
  const read = (path: string) => readFileSync(path, 'utf8').replaceAll('\r\n', '\n')
  const job =
    read('.github/workflows/rust.yml')
      .split('\n  matching-fixture-performance:\n')[1]
      ?.split(/\n {2}(?=[\w-]+:\n)/, 1)[0] ?? ''
  const install =
    job.split('- name: Install performance dependencies\n')[1]?.split('\n      - ', 1)[0] ?? ''
  const compare =
    job
      .split('- name: Compare native and Electron matching fixtures\n')[1]
      ?.split('\n      - ', 1)[0] ?? ''
  const packages = (
    install.match(
      /\n {12}buddy_apt sudo install -y --no-install-recommends \\\n((?:[^\n]*\\\n)*[^\n]*)\n/
    )?.[1] ?? ''
  )
    .split(/[\s\\]+/)
    .filter(Boolean)
  // The native .deb's own runtime declaration, first alternative of each
  // relation. libc6, libgcc-s1 and libstdc++6 come with every runner image
  // (apt itself needs them), and naming them could upgrade the C library.
  const baseSystem = ['libc6', 'libgcc-s1', 'libstdc++6']
  const declared = (
    JSON.parse(read('rust/desktop-package.json')) as { linuxDependencies: string[] }
  ).linuxDependencies.map(relation => relation.split('|')[0].split('(')[0].trim())
  const nativeRuntime = declared.filter(name => !baseSystem.includes(name))
  const harness = ['xvfb', 'openbox']

  it('runs a downloaded native binary instead of building one', () => {
    expect(job).toContain('name: buddy-performance-binary-${{ matrix.os }}')
    expect(job).toContain('path: rust/target/release')
    expect(job).not.toMatch(/\bcargo\s/)
    expect(packages.length).toBeGreaterThan(0)
  })

  it('installs no build SDK packages', () => {
    expect(packages.filter(name => /-dev(?:-bin)?$/.test(name))).toEqual([])
  })

  it('installs exactly the native runtime the .deb declares plus the display harness', () => {
    expect(baseSystem.every(name => declared.includes(name))).toBe(true)
    for (const name of nativeRuntime) expect(packages, name).toContain(name)
    for (const name of baseSystem) expect(packages, name).not.toContain(name)
    expect([...packages].sort()).toEqual([...nativeRuntime, ...harness].sort())
  })

  it('installs every display tool the comparison launches', () => {
    expect(compare).toContain(
      "xvfb-run -a -s '-screen 0 1600x1000x24' dbus-run-session -- bash -c '"
    )
    expect(compare).toContain('openbox >"$RUNNER_TEMP/performance-openbox.log" 2>&1 &')
    for (const name of harness) expect(packages, name).toContain(name)
  })

  it('keeps the install order, helper and bounds', () => {
    expect(install).toContain('timeout-minutes: 45\n')
    const order = [
      'bun install --frozen-lockfile',
      'if [[ "$RUNNER_OS" == Linux ]]; then',
      'source scripts/lib/bounded-apt.sh',
      'buddy_apt sudo update',
      'buddy_apt sudo install -y --no-install-recommends',
      'if [[ "$RUNNER_OS" != Windows ]]; then chmod +x rust/target/release/buddy; fi',
    ].map(line => install.indexOf(line))
    expect(order.every(index => index > -1)).toBe(true)
    expect(order).toEqual([...order].sort((a, b) => a - b))
    expect(install.match(/buddy_apt sudo (?:update|install)/g)).toEqual([
      'buddy_apt sudo update',
      'buddy_apt sudo install',
    ])
  })
})
