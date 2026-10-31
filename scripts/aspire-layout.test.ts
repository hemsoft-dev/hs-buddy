import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const repoRoot = resolve(import.meta.dirname, '..')

async function readJson(path: string): Promise<Record<string, unknown>> {
  return JSON.parse(await readFile(resolve(repoRoot, path), 'utf8')) as Record<string, unknown>
}

describe('Aspire AppHost isolation', () => {
  it('keeps the AppHost in its own Bun package', async () => {
    const config = await readJson('aspire.config.json')
    const rootPackage = await readJson('package.json')
    const appHostPackage = await readJson('aspire-apphost/package.json')

    expect(config.appHost).toMatchObject({
      path: 'aspire-apphost/apphost.mts',
      language: 'typescript/nodejs',
    })
    expect(rootPackage.devDependencies).not.toHaveProperty('vscode-jsonrpc')
    expect(appHostPackage).toMatchObject({
      private: true,
      packageManager: rootPackage.packageManager,
      engines: { node: '>=22.0.0' },
      dependencies: { 'vscode-jsonrpc': '^9.0.1' },
      devDependencies: { '@types/node': '^22.0.0' },
    })
  })

  it('runs application resources from the parent workspace without installing it', async () => {
    const appHost = await readFile(resolve(repoRoot, 'aspire-apphost/apphost.mts'), 'utf8')

    expect(appHost).toContain("from './.aspire/modules/aspire.mjs'")
    expect(appHost.match(/\.add(?:JavaScript|Vite)App\('[^']+', '\.\.'\)/g)).toHaveLength(3)
    expect(appHost.match(/\.withBun\(\{ install: false \}\)/g)).toHaveLength(3)
    expect(appHost).toContain("if (process.env.BUDDY_ASPIRE_NATIVE === '1')")
    expect(appHost).toContain(".withRunScript('native:dev')")
  })

  it('keeps fixed profile ports out of the dynamic client range', async () => {
    const config = await readJson('aspire.config.json')
    const profiles = config.profiles as Record<string, Record<string, unknown>>
    const profile = profiles.https
    const urls = [
      ...(profile.applicationUrl as string).split(';'),
      ...Object.values(profile.environmentVariables as Record<string, string>),
    ]
    const ports = urls.map(url => Number(new URL(url).port))

    expect(new Set(ports).size).toBe(ports.length)
    expect(ports.every(port => port >= 1024 && port < 49_152)).toBe(true)
  })

  it('keeps the launcher fast path restore-free', async () => {
    const launcher = await readFile(resolve(repoRoot, 'scripts/runAspire.debug.ps1'), 'utf8')

    expect(launcher).toContain('aspire-apphost/.aspire/modules/aspire.mts')
    expect(launcher).toContain('aspire-apphost/node_modules/vscode-jsonrpc/package.json')
    expect(launcher).toContain("$aspireArgs += '--no-build'")
    expect(launcher).toContain('if (-not $FullBuild)')
    expect(launcher).toContain('Bootstrap with:')
    expect(launcher).toContain('ERROR: Application dependencies not found.')
    expect(launcher).toContain("@('scripts/checkApplicationDependencies.ts')")
    expect(launcher).toContain("$dependencyCheckArgs += '--repair'")
    expect(launcher.indexOf('& $bunCmd.Source @dependencyCheckArgs')).toBeLessThan(
      launcher.indexOf('# -- Kill orphaned Convex processes --')
    )
  })

  it('provides one-time setup with a fast typecheck preflight', async () => {
    const rootPackage = await readJson('package.json')
    const scripts = rootPackage.scripts as Record<string, string>
    const setup = await readFile(resolve(repoRoot, 'scripts/setup.ts'), 'utf8')

    expect(scripts.setup).toBe('bun scripts/setup.ts')
    expect(scripts.pretypecheck).toBe('bun scripts/checkAspireBootstrap.ts')
    expect(scripts.typecheck).not.toContain('aspire restore')
    expect(setup).toContain("['install', '--frozen-lockfile']")
    expect(setup).toContain("['restore', '--non-interactive']")
  })

  it('requires distinct ports for every measured startup milestone', async () => {
    const measurement = await readFile(
      resolve(repoRoot, 'scripts/measureAspireStartup.ps1'),
      'utf8'
    )

    expect(measurement).toContain('Group-Object Value')
    expect(measurement).toContain('Startup milestones must use distinct ports:')
    expect(measurement).toContain('-Ports $milestonePorts')
  })

  it('excludes generated SDK code from linting', async () => {
    const eslintConfig = await readFile(resolve(repoRoot, 'eslint.config.js'), 'utf8')

    expect(eslintConfig).toContain("'aspire-apphost/.aspire/**'")
  })
})
