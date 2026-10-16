import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

// Immutable release tags verified against actions/setup-node's Git refs.
// Updating the action also requires recording the verified tag here.
const setupNodeReleases: Record<string, string> = {
  '820762786026740c76f36085b0efc47a31fe5020': 'v7.0.0',
  '949feb2413d6458794dcd2491c4babbbce0c15c1': 'v7.1.0',
}
const directory = '.github/workflows'
const workflows = readdirSync(directory).filter(
  name => name.endsWith('.yml') && !name.endsWith('.lock.yml')
)

describe('handwritten workflow action pins', () => {
  it('uses the declared Bun version in CI and the Aspire workspace', () => {
    const rootPackage = JSON.parse(readFileSync('package.json', 'utf8')) as {
      packageManager: string
    }
    const appHost = JSON.parse(readFileSync('aspire-apphost/package.json', 'utf8')) as {
      packageManager: string
    }
    const setup = readFileSync('.github/actions/setup-bun/action.yml', 'utf8')
    const version = rootPackage.packageManager.match(/^bun@(\d+\.\d+\.\d+)$/)?.[1]
    expect(version).toBeDefined()
    expect(setup.match(/^ {4}default: (\S+)$/m)?.[1]).toBe(version)
    expect(appHost.packageManager).toBe(rootPackage.packageManager)
  })

  it('records the verified setup-node release beside every immutable pin', () => {
    let count = 0
    for (const file of workflows) {
      const source = readFileSync(join(directory, file), 'utf8')
      for (const match of source.matchAll(/uses: actions\/setup-node@([^\s]+)([^\r\n]*)/g)) {
        const [, sha, suffix] = match
        const release = setupNodeReleases[sha]
        expect(release, `${file}: unverified setup-node release ${sha}`).toBeDefined()
        expect(suffix.trim(), `${file}: ${sha} is ${release}`).toBe(`# ${release}`)
        count += 1
      }
    }
    expect(count).toBeGreaterThan(0)
  })
})
