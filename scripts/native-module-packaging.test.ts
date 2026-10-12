import { readFileSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { describe, expect, it } from 'vitest'
import { electronNativeExternals, electronRequireShimOutput } from '../vite.config'

interface PackageManifest {
  dependencies?: Record<string, string>
}

const packageManifest = JSON.parse(readFileSync('package.json', 'utf8')) as PackageManifest
const builderConfig = readFileSync('electron-builder.json5', 'utf8')

const nativeModules = [
  {
    dependency: 'node-pty',
    unpackPattern: 'node_modules/node-pty/**/*',
  },
  {
    dependency: 'koffi',
    unpackPattern: 'node_modules/@koromix/koffi-*/**/*',
  },
] as const

describe('Electron native module packaging', () => {
  it('loads a minified ESM chunk with the production require shim', async () => {
    const directory = mkdtempSync(join(tmpdir(), 'buddy-require-shim-'))
    try {
      const input = join(directory, 'input.mjs')
      writeFileSync(
        input,
        `import { createRequire } from 'node:module';
        const require = createRequire(import.meta.url);
        export const loadPath = () => require('node:path').basename('a/b');`
      )
      const result = await build({
        configFile: false,
        logLevel: 'silent',
        build: {
          write: false,
          minify: true,
          rolldownOptions: {
            input,
            external: ['node:module', 'node:path'],
            preserveEntrySignatures: 'strict',
            output: electronRequireShimOutput,
          },
        },
      })
      if (Array.isArray(result) || !('output' in result)) throw new Error('Missing bundle')
      const chunk = result.output.find(entry => entry.type === 'chunk')
      if (!chunk || chunk.type !== 'chunk') throw new Error('Missing chunk')
      const output = join(directory, 'output.mjs')
      writeFileSync(output, chunk.code)
      const probe = spawnSync(
        'node',
        [
          '--input-type=module',
          '--eval',
          `const m = await import(process.argv[1]); if(m.loadPath() !== 'b') throw new Error('require failed')`,
          pathToFileURL(output).href,
        ],
        { encoding: 'utf8', timeout: 5000 }
      )
      expect(probe.status, probe.stderr).toBe(0)
    } finally {
      rmSync(directory, { recursive: true, force: true })
    }
  })
  it.each(nativeModules)(
    'keeps $dependency external, resolvable, and unpacked',
    ({ dependency, unpackPattern }) => {
      expect(electronNativeExternals).toContain(dependency)
      expect(packageManifest.dependencies).toHaveProperty(dependency)
      expect(builderConfig).toMatch(new RegExp(`["']${unpackPattern.replaceAll('*', '\\*')}["']`))
    }
  )
})
