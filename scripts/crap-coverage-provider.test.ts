import { describe, expect, it } from 'vitest'
import type { CoverageProvider } from 'vitest/node'
import crapCoverageProvider, { withoutSourceMapRemap } from './crap-coverage-provider'

type Hooks = {
  name: string
  setInputSourceMap: (...args: unknown[]) => void
  getCoverageMapForUncoveredFiles: (
    files: string[]
  ) => Promise<{ coverageMap: unknown; inputSourceMaps: Map<string, unknown> }>
}

function wrappedFake() {
  const stored: unknown[] = []
  const fake: Hooks = {
    name: 'istanbul',
    setInputSourceMap: (...args) => void stored.push(args),
    getCoverageMapForUncoveredFiles: async files => ({
      coverageMap: { covered: files },
      inputSourceMaps: new Map([['src/a.ts', { version: 3 }]]),
    }),
  }
  const wrapped = withoutSourceMapRemap(fake as unknown as CoverageProvider) as unknown as Hooks
  return { wrapped, stored }
}

describe('CRAP coverage provider', () => {
  it('stops storing source maps for loaded files', () => {
    const { wrapped, stored } = wrappedFake()
    wrapped.setInputSourceMap('environment', 'src/a.ts', { version: 3 })
    expect(stored).toEqual([])
  })

  it('keeps uncovered-file coverage but drops its source maps', async () => {
    const { wrapped } = wrappedFake()
    const result = await wrapped.getCoverageMapForUncoveredFiles(['src/b.ts'])
    expect(result.coverageMap).toEqual({ covered: ['src/b.ts'] })
    expect(result.inputSourceMaps.size).toBe(0)
  })

  it('fails closed when the provider no longer has the source-map hooks', () => {
    const bare = { name: 'istanbul' } as unknown as CoverageProvider
    expect(() => withoutSourceMapRemap(bare)).toThrow('source-map hooks changed')
  })

  it('wraps the installed Istanbul provider', async () => {
    const provider = (await crapCoverageProvider.getProvider()) as unknown as Hooks
    expect(provider.name).toBe('istanbul')
    expect(typeof provider.getCoverageMapForUncoveredFiles).toBe('function')
  })
})
