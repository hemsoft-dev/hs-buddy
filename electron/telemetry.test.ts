import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

const telemetryState = vi.hoisted(() => {
  const mockSpan = {
    setStatus: vi.fn(),
    recordException: vi.fn(),
    end: vi.fn(),
  }
  const mockCounter = { add: vi.fn() }
  const mockHistogram = { record: vi.fn() }
  const mockCreateCounter = vi.fn(() => mockCounter)
  const mockCreateHistogram = vi.fn(() => mockHistogram)
  const mockGetMeter = vi.fn(() => ({
    createCounter: mockCreateCounter,
    createHistogram: mockCreateHistogram,
  }))

  return {
    mockSpan,
    mockCounter,
    mockHistogram,
    mockStartActiveSpan: vi.fn((_name, _opts, fn) => fn(mockSpan)),
    mockCreateCounter,
    mockCreateHistogram,
    mockGetMeter,
    mockEmit: vi.fn(),
    mockDiagSetLogger: vi.fn(),
    mockSetGlobalLoggerProvider: vi.fn(),
    mockNodeSdkConstructor: vi.fn(),
    mockNodeSdkStart: vi.fn(),
    mockNodeSdkShutdown: vi.fn(async () => undefined),
    mockTraceExporterConstructor: vi.fn(),
    mockMetricExporterConstructor: vi.fn(),
    mockLogExporterConstructor: vi.fn(),
    mockMetricReaderConstructor: vi.fn(),
    mockBatchLogRecordProcessorConstructor: vi.fn(),
    mockLoggerProviderConstructor: vi.fn(),
    mockLoggerProviderShutdown: vi.fn(async () => undefined),
    mockHttpInstrumentationConstructor: vi.fn(),
    mockDnsInstrumentationConstructor: vi.fn(),
    mockResourceFromAttributes: vi.fn(attributes => ({ attributes })),
    sdkNodeImportError: null as Error | null,
  }
})

vi.mock('@opentelemetry/api', () => ({
  trace: { getTracer: vi.fn(() => ({ startActiveSpan: telemetryState.mockStartActiveSpan })) },
  metrics: {
    getMeter: telemetryState.mockGetMeter,
  },
  SpanStatusCode: { OK: 1, ERROR: 2 },
  diag: { setLogger: telemetryState.mockDiagSetLogger },
  DiagConsoleLogger: vi.fn(),
  DiagLogLevel: { DEBUG: 0 },
  logs: {
    getLogger: vi.fn(() => ({ emit: telemetryState.mockEmit })),
    setGlobalLoggerProvider: telemetryState.mockSetGlobalLoggerProvider,
  },
  SeverityNumber: { DEBUG: 5, INFO: 9, WARN: 13, ERROR: 17 },
}))

function buildSdkNodeModule() {
  if (telemetryState.sdkNodeImportError) throw telemetryState.sdkNodeImportError

  class NodeSDK {
    constructor(config: unknown) {
      telemetryState.mockNodeSdkConstructor(config)
    }

    start(): void {
      telemetryState.mockNodeSdkStart()
    }

    shutdown(): Promise<void> {
      return telemetryState.mockNodeSdkShutdown()
    }
  }

  return { NodeSDK }
}

vi.mock('@opentelemetry/sdk-node', buildSdkNodeModule)

vi.mock('@opentelemetry/exporter-trace-otlp-proto', () => ({
  OTLPTraceExporter: class {
    constructor(config: unknown) {
      telemetryState.mockTraceExporterConstructor(config)
    }
  },
}))

vi.mock('@opentelemetry/exporter-metrics-otlp-proto', () => ({
  OTLPMetricExporter: class {
    constructor(config: unknown) {
      telemetryState.mockMetricExporterConstructor(config)
    }
  },
}))

vi.mock('@opentelemetry/exporter-logs-otlp-proto', () => ({
  OTLPLogExporter: class {
    constructor(config: unknown) {
      telemetryState.mockLogExporterConstructor(config)
    }
  },
}))

vi.mock('@opentelemetry/sdk-metrics', () => ({
  PeriodicExportingMetricReader: class {
    constructor(config: unknown) {
      telemetryState.mockMetricReaderConstructor(config)
    }
  },
}))

function buildSdkLogsModule() {
  return {
    BatchLogRecordProcessor: class {
      constructor(options: unknown) {
        telemetryState.mockBatchLogRecordProcessorConstructor(options)
      }
    },
    LoggerProvider: class {
      constructor(config: unknown) {
        telemetryState.mockLoggerProviderConstructor(config)
      }

      shutdown(): Promise<void> {
        return telemetryState.mockLoggerProviderShutdown()
      }
    },
  }
}

vi.mock('@opentelemetry/sdk-logs', buildSdkLogsModule)

vi.mock('@opentelemetry/instrumentation-http', () => ({
  HttpInstrumentation: class {
    constructor() {
      telemetryState.mockHttpInstrumentationConstructor()
    }
  },
}))

vi.mock('@opentelemetry/instrumentation-dns', () => ({
  DnsInstrumentation: class {
    constructor() {
      telemetryState.mockDnsInstrumentationConstructor()
    }
  },
}))

vi.mock('@opentelemetry/resources', () => ({
  resourceFromAttributes: telemetryState.mockResourceFromAttributes,
}))

const importTelemetry = () => import('./telemetry')

describe('telemetry', () => {
  beforeEach(() => {
    vi.resetModules()
    vi.doMock('@opentelemetry/sdk-node', buildSdkNodeModule)
    vi.doMock('@opentelemetry/sdk-logs', buildSdkLogsModule)
    vi.clearAllMocks()
    vi.unstubAllEnvs()
    telemetryState.sdkNodeImportError = null
    telemetryState.mockNodeSdkShutdown.mockImplementation(async () => undefined)
    telemetryState.mockLoggerProviderShutdown.mockImplementation(async () => undefined)
    telemetryState.mockStartActiveSpan.mockImplementation((_name, _opts, fn) =>
      fn(telemetryState.mockSpan)
    )
  })

  afterEach(() => {
    vi.unstubAllEnvs()
    vi.restoreAllMocks()
  })

  describe('recordIpcCall', () => {
    it('does not throw when counters are uninitialized', async () => {
      const { recordIpcCall } = await importTelemetry()
      expect(() => recordIpcCall('test:channel', 42)).not.toThrow()
    })

    it('does not throw with error flag', async () => {
      const { recordIpcCall } = await importTelemetry()
      expect(() => recordIpcCall('test:channel', 100, true)).not.toThrow()
    })
  })

  describe('recordWindowOpen', () => {
    it('does not throw when counter is uninitialized', async () => {
      const { recordWindowOpen } = await importTelemetry()
      expect(() => recordWindowOpen('example.com')).not.toThrow()
    })
  })

  describe('emitLog', () => {
    it('emits a log record with correct severity for INFO', async () => {
      const { emitLog } = await importTelemetry()
      emitLog('INFO', 'Test message', { key: 'value' })
      expect(telemetryState.mockEmit).toHaveBeenCalledWith({
        severityNumber: 9,
        severityText: 'INFO',
        body: 'Test message',
        attributes: { key: 'value' },
      })
    })

    it('maps DEBUG severity number correctly', async () => {
      const { emitLog } = await importTelemetry()
      emitLog('DEBUG', 'debug msg')
      expect(telemetryState.mockEmit).toHaveBeenCalledWith(
        expect.objectContaining({ severityNumber: 5, severityText: 'DEBUG' })
      )
    })

    it('maps WARN severity number correctly', async () => {
      const { emitLog } = await importTelemetry()
      emitLog('WARN', 'warning msg')
      expect(telemetryState.mockEmit).toHaveBeenCalledWith(
        expect.objectContaining({ severityNumber: 13, severityText: 'WARN' })
      )
    })

    it('maps ERROR severity number correctly', async () => {
      const { emitLog } = await importTelemetry()
      emitLog('ERROR', 'error msg')
      expect(telemetryState.mockEmit).toHaveBeenCalledWith(
        expect.objectContaining({ severityNumber: 17, severityText: 'ERROR' })
      )
    })

    it('passes undefined attributes when not provided', async () => {
      const { emitLog } = await importTelemetry()
      emitLog('INFO', 'bare message')
      expect(telemetryState.mockEmit).toHaveBeenCalledWith(
        expect.objectContaining({ body: 'bare message', attributes: undefined })
      )
    })

    it('falls back to INFO severity for unknown severity levels', async () => {
      const { emitLog: emitLogFn } = await importTelemetry()
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      emitLogFn('TRACE' as any, 'trace msg')
      expect(telemetryState.mockEmit).toHaveBeenCalledWith(
        expect.objectContaining({ severityNumber: 9, severityText: 'TRACE' })
      )
    })
  })

  describe('withSpan', () => {
    it('calls startActiveSpan with span name and attributes', async () => {
      const { withSpan } = await importTelemetry()
      await withSpan('test-op', { foo: 'bar' }, async () => 'ok')
      expect(telemetryState.mockStartActiveSpan).toHaveBeenCalledWith(
        'test-op',
        { attributes: { foo: 'bar' } },
        expect.any(Function)
      )
    })

    it('sets span status to OK on success', async () => {
      const { withSpan } = await importTelemetry()
      const result = await withSpan('success-op', {}, async () => 42)
      expect(result).toBe(42)
      expect(telemetryState.mockSpan.setStatus).toHaveBeenCalledWith({ code: 1 })
    })

    it('sets span status to ERROR and records exception on failure', async () => {
      const { withSpan } = await importTelemetry()
      const error = new Error('test failure')
      await expect(
        withSpan('fail-op', {}, async () => {
          throw error
        })
      ).rejects.toThrow('test failure')

      expect(telemetryState.mockSpan.setStatus).toHaveBeenCalledWith({
        code: 2,
        message: 'Error: test failure',
      })
      expect(telemetryState.mockSpan.recordException).toHaveBeenCalledWith(error)
    })

    it('always calls span.end()', async () => {
      const { withSpan } = await importTelemetry()
      await withSpan('end-op', {}, async () => 'done')
      expect(telemetryState.mockSpan.end).toHaveBeenCalled()
    })

    it('calls span.end() even on failure', async () => {
      const { withSpan } = await importTelemetry()
      try {
        await withSpan('fail-end', {}, async () => {
          throw new Error('boom')
        })
      } catch (_: unknown) {
        // expected
      }
      expect(telemetryState.mockSpan.end).toHaveBeenCalled()
    })

    it('records non-Error exceptions wrapped in Error', async () => {
      const { withSpan } = await importTelemetry()
      await expect(
        withSpan('string-throw', {}, async () => {
          throw 'string error'
        })
      ).rejects.toBe('string error')
      expect(telemetryState.mockSpan.recordException).toHaveBeenCalledWith(
        new Error('string error')
      )
    })

    it('handles a fn that returns undefined', async () => {
      const { withSpan: withSpanFn } = await importTelemetry()
      const result = await withSpanFn('void-op', {}, async () => undefined)
      expect(result).toBeUndefined()
      expect(telemetryState.mockSpan.setStatus).toHaveBeenCalledWith({ code: 1 })
      expect(telemetryState.mockSpan.end).toHaveBeenCalled()
    })
  })

  describe('initTelemetry', () => {
    it('logs and returns when OTEL_EXPORTER_OTLP_ENDPOINT is not set', async () => {
      const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
      const { initTelemetry } = await importTelemetry()

      await expect(initTelemetry()).resolves.toBeUndefined()

      expect(logSpy).toHaveBeenCalledWith(
        '[Telemetry] OTEL_EXPORTER_OTLP_ENDPOINT not set — telemetry disabled'
      )
      expect(telemetryState.mockNodeSdkConstructor).not.toHaveBeenCalled()
      expect(telemetryState.mockSetGlobalLoggerProvider).not.toHaveBeenCalled()
    })

    it('is idempotent after the SDK has already been initialized', async () => {
      vi.stubEnv('OTEL_EXPORTER_OTLP_ENDPOINT', 'http://otel.local')
      const { initTelemetry } = await importTelemetry()

      await initTelemetry()
      await initTelemetry()

      expect(telemetryState.mockNodeSdkConstructor).toHaveBeenCalledTimes(1)
      expect(telemetryState.mockNodeSdkStart).toHaveBeenCalledTimes(1)
      expect(telemetryState.mockGetMeter).toHaveBeenCalledTimes(1)
    })

    it('warns without throwing when a dynamic SDK import fails', async () => {
      vi.stubEnv('OTEL_EXPORTER_OTLP_ENDPOINT', 'http://otel.local')
      const importError = new Error('import failed')
      telemetryState.sdkNodeImportError = importError
      vi.resetModules()
      vi.doMock('@opentelemetry/sdk-node', () => {
        throw importError
      })
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})
      const { initTelemetry } = await importTelemetry()

      await expect(initTelemetry()).resolves.toBeUndefined()

      expect(warnSpy).toHaveBeenCalledWith(
        '[Telemetry] Failed to load SDK packages (non-fatal):',
        expect.objectContaining({ cause: importError })
      )
      expect(telemetryState.mockNodeSdkConstructor).not.toHaveBeenCalled()
    })

    it('initializes exporters, logger provider, and metric handles when telemetry is enabled', async () => {
      vi.stubEnv('OTEL_EXPORTER_OTLP_ENDPOINT', 'http://otel.local')
      vi.stubEnv('OTEL_SERVICE_NAME', 'buddy-main')
      vi.stubEnv('npm_package_version', '1.2.3')
      vi.stubEnv('OTEL_LOG_LEVEL', 'debug')
      const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
      const { initTelemetry, recordIpcCall, recordWindowOpen } = await importTelemetry()

      await initTelemetry()
      recordIpcCall('ipc:test', 42, true)
      recordWindowOpen('main')

      expect(telemetryState.mockDiagSetLogger).toHaveBeenCalledWith(expect.anything(), 0)
      expect(telemetryState.mockResourceFromAttributes).toHaveBeenCalledWith({
        'service.name': 'buddy-main',
        'service.version': '1.2.3',
      })
      expect(telemetryState.mockLogExporterConstructor).toHaveBeenCalledWith({
        url: 'http://otel.local/v1/logs',
      })
      expect(telemetryState.mockTraceExporterConstructor).toHaveBeenCalledWith({
        url: 'http://otel.local/v1/traces',
      })
      expect(telemetryState.mockMetricExporterConstructor).toHaveBeenCalledWith({
        url: 'http://otel.local/v1/metrics',
      })
      expect(telemetryState.mockMetricReaderConstructor).toHaveBeenCalledWith(
        expect.objectContaining({ exportIntervalMillis: 15_000, exporter: expect.any(Object) })
      )
      expect(telemetryState.mockBatchLogRecordProcessorConstructor).toHaveBeenCalledWith(
        expect.objectContaining({ exporter: expect.any(Object) })
      )
      expect(telemetryState.mockLoggerProviderConstructor).toHaveBeenCalledWith(
        expect.objectContaining({
          resource: expect.any(Object),
          processors: [expect.any(Object)],
        })
      )
      expect(telemetryState.mockSetGlobalLoggerProvider).toHaveBeenCalledTimes(1)
      expect(telemetryState.mockHttpInstrumentationConstructor).toHaveBeenCalledTimes(1)
      expect(telemetryState.mockDnsInstrumentationConstructor).toHaveBeenCalledTimes(1)
      expect(telemetryState.mockNodeSdkConstructor).toHaveBeenCalledWith(
        expect.objectContaining({
          resource: expect.any(Object),
          traceExporter: expect.any(Object),
          metricReaders: [expect.any(Object)],
          instrumentations: [expect.any(Object), expect.any(Object)],
        })
      )
      expect(telemetryState.mockNodeSdkStart).toHaveBeenCalledTimes(1)
      expect(telemetryState.mockGetMeter).toHaveBeenCalledWith('buddy-main', '1.2.3')
      expect(telemetryState.mockCreateCounter).toHaveBeenNthCalledWith(1, 'buddy.ipc.calls', {
        description: 'Number of IPC handler invocations',
        unit: '{calls}',
      })
      expect(telemetryState.mockCreateHistogram).toHaveBeenCalledWith('buddy.ipc.duration', {
        description: 'IPC handler execution time',
        unit: 'ms',
      })
      expect(telemetryState.mockCreateCounter).toHaveBeenNthCalledWith(2, 'buddy.ipc.errors', {
        description: 'Number of IPC handler errors',
        unit: '{errors}',
      })
      expect(telemetryState.mockCreateCounter).toHaveBeenNthCalledWith(3, 'buddy.windows.opened', {
        description: 'Number of browser windows opened',
        unit: '{windows}',
      })
      expect(telemetryState.mockCounter.add).toHaveBeenCalledTimes(3)
      expect(telemetryState.mockHistogram.record).toHaveBeenCalledWith(42, {
        'ipc.channel': 'ipc:test',
      })
      expect(logSpy).toHaveBeenCalledWith(
        "[Telemetry] Initialized — exporting to http://otel.local as 'buddy-main'"
      )
    })
  })

  describe('shutdownTelemetry', () => {
    it('resolves without error when sdk is null', async () => {
      const { shutdownTelemetry } = await importTelemetry()
      await expect(shutdownTelemetry()).resolves.toBeUndefined()
    })

    it('warns when provider shutdown throws', async () => {
      vi.stubEnv('OTEL_EXPORTER_OTLP_ENDPOINT', 'http://otel.local')
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})
      const { initTelemetry, shutdownTelemetry } = await importTelemetry()

      await initTelemetry()
      telemetryState.mockLoggerProviderShutdown.mockRejectedValueOnce(new Error('shutdown failed'))

      await expect(shutdownTelemetry()).resolves.toBeUndefined()

      expect(warnSpy).toHaveBeenCalledWith('[Telemetry] Shutdown error:', expect.any(Error))
    })
  })

  describe('initTelemetry', () => {
    it('logs disabled message when OTEL_EXPORTER_OTLP_ENDPOINT is not set', async () => {
      const consoleSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
      const originalEndpoint = process.env.OTEL_EXPORTER_OTLP_ENDPOINT
      delete process.env.OTEL_EXPORTER_OTLP_ENDPOINT
      const { initTelemetry: initFresh } = await import('./telemetry')
      await initFresh()
      expect(consoleSpy).toHaveBeenCalledWith(expect.stringContaining('telemetry disabled'))
      consoleSpy.mockRestore()
      if (originalEndpoint === undefined) {
        delete process.env.OTEL_EXPORTER_OTLP_ENDPOINT
      } else {
        process.env.OTEL_EXPORTER_OTLP_ENDPOINT = originalEndpoint
      }
    })

    it('initializes SDK when OTEL_EXPORTER_OTLP_ENDPOINT is set', async () => {
      process.env.OTEL_EXPORTER_OTLP_ENDPOINT = 'http://localhost:4318'
      const consoleSpy = vi.spyOn(console, 'log').mockImplementation(() => {})

      const { initTelemetry: initEnabled } = await importTelemetry()
      await initEnabled()

      expect(telemetryState.mockNodeSdkStart).toHaveBeenCalled()
      expect(consoleSpy).toHaveBeenCalledWith(expect.stringContaining('Initialized'))
      consoleSpy.mockRestore()
      delete process.env.OTEL_EXPORTER_OTLP_ENDPOINT
    })

    it('handles SDK import failure gracefully', async () => {
      vi.resetModules()

      vi.doMock('@opentelemetry/api', () => ({
        trace: { getTracer: vi.fn(() => ({ startActiveSpan: vi.fn() })) },
        metrics: { getMeter: vi.fn() },
        SpanStatusCode: { OK: 1, ERROR: 2 },
        diag: { setLogger: vi.fn() },
        DiagConsoleLogger: vi.fn(),
        DiagLogLevel: { DEBUG: 0 },
        logs: { getLogger: vi.fn(() => ({ emit: vi.fn() })), setGlobalLoggerProvider: vi.fn() },
        SeverityNumber: { DEBUG: 5, INFO: 9, WARN: 13, ERROR: 17 },
      }))
      // Make the SDK import throw
      vi.doMock('@opentelemetry/sdk-node', () => {
        throw new Error('SDK not found')
      })

      process.env.OTEL_EXPORTER_OTLP_ENDPOINT = 'http://localhost:4318'
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})

      const { initTelemetry: initFail } = await import('./telemetry')
      await initFail()

      expect(warnSpy).toHaveBeenCalledWith(
        expect.stringContaining('Failed to load SDK'),
        expect.anything()
      )
      warnSpy.mockRestore()
      delete process.env.OTEL_EXPORTER_OTLP_ENDPOINT
    })

    it('enables debug diagnostics when OTEL_LOG_LEVEL=debug', async () => {
      vi.resetModules()

      const mockDiag = { setLogger: vi.fn() }
      const mockNodeSDK = { start: vi.fn(), shutdown: vi.fn().mockResolvedValue(undefined) }
      const mockLoggerProvider = { shutdown: vi.fn().mockResolvedValue(undefined) }

      // Use regular functions for constructors (arrow functions can't be used with `new`)
      function MockNodeSDK2() {
        return mockNodeSDK
      }
      function MockLoggerProvider2() {
        return mockLoggerProvider
      }

      vi.doMock('@opentelemetry/api', () => ({
        trace: { getTracer: vi.fn(() => ({ startActiveSpan: vi.fn() })) },
        metrics: {
          getMeter: vi.fn(() => ({
            createCounter: vi.fn(() => ({ add: vi.fn() })),
            createHistogram: vi.fn(() => ({ record: vi.fn() })),
          })),
        },
        SpanStatusCode: { OK: 1, ERROR: 2 },
        diag: mockDiag,
        DiagConsoleLogger: vi.fn(),
        DiagLogLevel: { DEBUG: 0 },
        logs: { getLogger: vi.fn(() => ({ emit: vi.fn() })), setGlobalLoggerProvider: vi.fn() },
        SeverityNumber: { DEBUG: 5, INFO: 9, WARN: 13, ERROR: 17 },
      }))
      vi.doMock('@opentelemetry/sdk-node', () => ({ NodeSDK: MockNodeSDK2 }))
      vi.doMock('@opentelemetry/exporter-trace-otlp-proto', () => ({ OTLPTraceExporter: vi.fn() }))
      vi.doMock('@opentelemetry/exporter-metrics-otlp-proto', () => ({
        OTLPMetricExporter: vi.fn(),
      }))
      vi.doMock('@opentelemetry/exporter-logs-otlp-proto', () => ({ OTLPLogExporter: vi.fn() }))
      vi.doMock('@opentelemetry/sdk-metrics', () => ({ PeriodicExportingMetricReader: vi.fn() }))
      vi.doMock('@opentelemetry/sdk-logs', () => ({
        BatchLogRecordProcessor: vi.fn(),
        LoggerProvider: MockLoggerProvider2,
      }))
      vi.doMock('@opentelemetry/instrumentation-http', () => ({ HttpInstrumentation: vi.fn() }))
      vi.doMock('@opentelemetry/instrumentation-dns', () => ({ DnsInstrumentation: vi.fn() }))
      vi.doMock('@opentelemetry/resources', () => ({ resourceFromAttributes: vi.fn(() => ({})) }))

      process.env.OTEL_EXPORTER_OTLP_ENDPOINT = 'http://localhost:4318'
      process.env.OTEL_LOG_LEVEL = 'debug'
      vi.spyOn(console, 'log').mockImplementation(() => {})

      const { initTelemetry: initDebug } = await import('./telemetry')
      await initDebug()

      expect(mockDiag.setLogger).toHaveBeenCalled()
      delete process.env.OTEL_EXPORTER_OTLP_ENDPOINT
      delete process.env.OTEL_LOG_LEVEL
    })
  })

  describe('shutdownTelemetry - with active SDK', () => {
    it('calls shutdown on loggerProvider and sdk', async () => {
      vi.stubEnv('OTEL_EXPORTER_OTLP_ENDPOINT', 'http://localhost:4318')
      vi.spyOn(console, 'log').mockImplementation(() => {})
      const { initTelemetry, shutdownTelemetry } = await importTelemetry()

      await initTelemetry()
      await shutdownTelemetry()

      expect(telemetryState.mockLoggerProviderShutdown).toHaveBeenCalled()
      expect(telemetryState.mockNodeSdkShutdown).toHaveBeenCalled()
    })

    it('catches shutdown errors without throwing', async () => {
      vi.stubEnv('OTEL_EXPORTER_OTLP_ENDPOINT', 'http://localhost:4318')
      vi.spyOn(console, 'log').mockImplementation(() => {})
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})
      const { initTelemetry, shutdownTelemetry } = await importTelemetry()

      await initTelemetry()
      telemetryState.mockLoggerProviderShutdown.mockRejectedValueOnce(
        new Error('logger shutdown fail')
      )

      await expect(shutdownTelemetry()).resolves.toBeUndefined()

      expect(warnSpy).toHaveBeenCalledWith(
        expect.stringContaining('Shutdown error'),
        expect.anything()
      )
    })
  })

  describe('initTelemetry', () => {
    it('is a no-op when OTEL_EXPORTER_OTLP_ENDPOINT is not set', async () => {
      const prev = process.env.OTEL_EXPORTER_OTLP_ENDPOINT
      delete process.env.OTEL_EXPORTER_OTLP_ENDPOINT
      const { initTelemetry } = await import('./telemetry')
      try {
        await expect(initTelemetry()).resolves.toBeUndefined()
      } finally {
        if (prev !== undefined) {
          process.env.OTEL_EXPORTER_OTLP_ENDPOINT = prev
        }
      }
    })
  })

  describe('recordIpcCall no-op metric path', () => {
    it('does not touch metric instruments before initialization', async () => {
      const { metrics } = await import('@opentelemetry/api')
      metrics.getMeter('test')

      const { recordIpcCall: recordIpcCallFn } = await importTelemetry()
      recordIpcCallFn('test:chan', 50, false)
      recordIpcCallFn('test:chan', 100, true)

      expect(telemetryState.mockCounter.add).not.toHaveBeenCalled()
      expect(telemetryState.mockHistogram.record).not.toHaveBeenCalled()
    })
  })
})
