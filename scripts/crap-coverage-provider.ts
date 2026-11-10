import istanbul from '@vitest/coverage-istanbul'
import type { CoverageProvider, CoverageProviderModule } from 'vitest/node'

type UncoveredFiles = { coverageMap: unknown; inputSourceMaps: Map<string, unknown> }
type RemappingProvider = CoverageProvider & {
  setInputSourceMap?: (...args: unknown[]) => void
  getCoverageMapForUncoveredFiles?: (coveredFiles: string[]) => Promise<UncoveredFiles>
}

/**
 * The CRAP instrumenter records coverage in original TypeScript positions
 * before Vite transforms a file. Since `@vitest/coverage-istanbul` 5.1, the
 * provider also keeps Vite's combined source map for each file and remaps all
 * coverage through it after the run. Remapping positions that are already
 * original drops every entry, so the report comes out empty. This provider
 * keeps Istanbul's collection, merging and reporting, and turns off only
 * that second remap. It fails closed if the provider's internals change.
 */
export function withoutSourceMapRemap(provider: RemappingProvider): CoverageProvider {
  const uncovered = provider.getCoverageMapForUncoveredFiles
  if (typeof provider.setInputSourceMap !== 'function' || typeof uncovered !== 'function')
    throw new Error('Unsupported Istanbul coverage provider: source-map hooks changed')
  provider.setInputSourceMap = () => {}
  provider.getCoverageMapForUncoveredFiles = async coveredFiles => {
    const result = await uncovered.call(provider, coveredFiles)
    return { coverageMap: result.coverageMap, inputSourceMaps: new Map() }
  }
  return provider
}

const crapCoverageProvider: CoverageProviderModule = {
  ...istanbul,
  async getProvider() {
    return withoutSourceMapRemap((await istanbul.getProvider()) as RemappingProvider)
  },
}

export default crapCoverageProvider
