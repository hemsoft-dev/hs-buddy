import { defineConfig, type Plugin } from 'vite'
import path from 'node:path'
import electron from 'vite-plugin-electron/simple'
import react from '@vitejs/plugin-react'

// Heavy OpenTelemetry SDK/exporter packages are externalized to avoid bundling
// ~2 MB of code that's only used during development (with Aspire).
// The lightweight API packages (@opentelemetry/api, api-logs) remain bundled
// since they provide no-op implementations when the SDK isn't loaded.
// Koffi must be a direct dependency so the external import remains resolvable.
import.meta.resolve('koffi')
export const electronNativeExternals = ['node-pty', 'koffi'] as const

const otelExternals = [
  '@opentelemetry/sdk-node',
  '@opentelemetry/exporter-trace-otlp-proto',
  '@opentelemetry/exporter-metrics-otlp-proto',
  '@opentelemetry/exporter-logs-otlp-proto',
  '@opentelemetry/sdk-metrics',
  '@opentelemetry/sdk-logs',
  '@opentelemetry/instrumentation-http',
  '@opentelemetry/instrumentation-dns',
  '@opentelemetry/resources',
]

// Vite 8 uses Rolldown which outputs ESM by default for "type":"module" projects.
// CJS dependencies bundled into the ESM main process lose access to `require`.
// Inject a Node.js-native require via createRequire so Rolldown's __require shim works.
const requireShim = [
  // postBanner is deliberately not minified; keep this tiny shim compact.
  "import{createRequire as __r}from'module';",
  'const require=__r(import.meta.url);',
].join('\n')

export const electronRequireShimOutput = { postBanner: requireShim }

// Keep splittable Electron chunks below Vite's default 500 kB budget. Shiki
// ships a few individual grammar modules that are larger than that threshold,
// so the renderer warning limit sits just above the largest grammar.
const MAX_SPLIT_CHUNK_SIZE = 450 * 1024
const SHIKI_GRAMMAR_WARNING_LIMIT_KB = 800
const DEVELOPMENT_SCRIPT_SRC = "script-src 'self' 'unsafe-inline' 'unsafe-eval'"
export const PRODUCTION_SCRIPT_SRC = "script-src 'self' 'wasm-unsafe-eval'"

/** Rolldown may derive the shared renderer chunk name from the checkout basename. */
export function rendererChunkFileName(
  chunk: { name: string; moduleIds: readonly string[] },
  rendererEntryModule: string
): string {
  const entry = rendererEntryModule.replaceAll('\\', '/')
  const isRoot = chunk.moduleIds.some(id => id.replaceAll('\\', '/') === entry)
  return `assets/${isRoot ? 'app' : chunk.name}-[hash].js`
}

export function hardenProductionCsp(html: string): string {
  const occurrences = html.split(DEVELOPMENT_SCRIPT_SRC).length - 1
  if (occurrences !== 1) {
    throw new Error(
      `Expected one development script-src directive before production hardening, found ${occurrences}`
    )
  }
  return html.replace(DEVELOPMENT_SCRIPT_SRC, PRODUCTION_SCRIPT_SRC)
}

function productionCspPlugin(): Plugin {
  return {
    name: 'production-csp',
    apply: 'build',
    enforce: 'post',
    transformIndexHtml: hardenProductionCsp,
  }
}

function codeSplittingGroups(sourcePattern: RegExp) {
  return {
    minSize: 20 * 1024,
    groups: [
      {
        name: 'vendor',
        test: (id: string) => {
          if (!/[\\/]node_modules[\\/]/.test(id)) return false
          return true
        },
        maxSize: MAX_SPLIT_CHUNK_SIZE,
        priority: 20,
      },
      {
        name: 'app',
        test: (id: string) => {
          if (!sourcePattern.test(id)) return false
          return true
        },
        maxSize: MAX_SPLIT_CHUNK_SIZE,
        priority: 10,
      },
    ],
  }
}

function rendererCodeSplitting() {
  return {
    groups: [
      {
        name: 'markdown-emoji',
        test: /[\\/]node_modules[\\/].*[\\/]gemoji[\\/]/,
        maxSize: MAX_SPLIT_CHUNK_SIZE,
        includeDependenciesRecursively: false,
        priority: 20,
      },
      {
        name: 'markdown-parser',
        test: /[\\/]node_modules[\\/].*[\\/](?:parse5|entities|refractor)[\\/]/,
        maxSize: MAX_SPLIT_CHUNK_SIZE,
        includeDependenciesRecursively: false,
        priority: 10,
      },
    ],
  }
}

// https://vitejs.dev/config/
export default defineConfig(({ mode }) => {
  // Browser E2E serves only the renderer and injects IPC mocks. Electron E2E
  // keeps the Electron plugin while sharing the deterministic Convex renderer mock.
  const isBrowserE2E = mode === 'e2e' || process.env.VITE_E2E === '1'
  const usesE2ERendererMocks = isBrowserE2E || mode === 'electron-e2e'

  return {
    // The renderer loads via file:// in production (win.loadFile), so asset
    // URLs must be relative. (vite-plugin-electron-renderer set this before
    // it was removed — see the electron() plugin options below.)
    base: './',
    // A local Electron E2E run must not load credentials from developer env files.
    envDir: mode === 'electron-e2e' ? false : undefined,
    // In E2E mode, alias convex/react to a mock module that provides no-op hooks.
    // This prevents the real Convex client from trying WebSocket connections.
    ...(usesE2ERendererMocks
      ? {
          resolve: {
            alias: {
              'convex/react': path.resolve(__dirname, 'testing/convex-react-mock.ts'),
            },
          },
        }
      : {}),
    build: {
      manifest: true,
      chunkSizeWarningLimit: SHIKI_GRAMMAR_WARNING_LIMIT_KB,
      rolldownOptions: {
        output: {
          // Let Rolldown's automatic chunking preserve every route-level
          // dynamic import. Broad app/vendor groups merge lazy modules back
          // into the entry graph because group matches include dependencies.
          codeSplitting: rendererCodeSplitting(),
          strictExecutionOrder: true,
          // Stabilize actual generated imports, not only bundle-report labels.
          chunkFileNames: chunk =>
            rendererChunkFileName(chunk, path.resolve(__dirname, 'src/main.tsx')),
        },
      },
    },
    plugins: [
      react(),
      productionCspPlugin(),
      ...(!isBrowserE2E
        ? [
            electron({
              main: {
                entry: 'electron/main.ts',
                vite: {
                  build: {
                    target: 'esnext',
                    rolldownOptions: {
                      // Native modules must retain their package-relative loaders.
                      // OTel SDK packages are dev-only (Aspire) and load at runtime.
                      external: [...electronNativeExternals, ...otelExternals],
                      output: {
                        // Keep the shim's bindings outside minification: a banner
                        // can reuse a minified name already declared by the chunk.
                        ...electronRequireShimOutput,
                        codeSplitting: codeSplittingGroups(/[\\/](?:electron|shared)[\\/]/),
                      },
                    },
                  },
                },
              },
              preload: {
                input: path.join(__dirname, 'electron/preload.ts'),
              },
            }),
          ]
        : []),
    ],
  }
})
