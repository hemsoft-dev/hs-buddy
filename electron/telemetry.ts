/**
 * OpenTelemetry instrumentation for the Electron main process.
 * Exports structured logs, traces (spans), and metrics to the Aspire
 * dashboard via OTLP/proto when OTEL_EXPORTER_OTLP_ENDPOINT is set.
 *
 * Heavy SDK packages are lazy-loaded via dynamic import() to avoid bundling
 * ~2 MB of code that's only used during development (with Aspire).
 * The lightweight API packages provide no-op implementations by default.
 *
 * Call `initTelemetry()` before any other imports that use HTTP/DNS
 * so the auto-instrumentations can patch them.
 */

import {
  trace,
  metrics,
  type Span,
  SpanStatusCode,
  type Meter,
  type Counter,
  type Histogram,
  diag,
  DiagConsoleLogger,
  DiagLogLevel,
  logs,
  SeverityNumber,
} from '@opentelemetry/api'

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

let sdk: { shutdown(): Promise<void> } | null = null
let loggerProvider: { shutdown(): Promise<void> } | null = null

// Metrics handles (lazy-initialized)
let ipcCallCounter: Counter | null = null
let ipcDurationHistogram: Histogram | null = null
let ipcErrorCounter: Counter | null = null
let windowOpenCounter: Counter | null = null

// ---------------------------------------------------------------------------
// Init / Shutdown
// ---------------------------------------------------------------------------

/**
 * Initialize OpenTelemetry if OTEL_EXPORTER_OTLP_ENDPOINT is set
 * (Aspire injects this automatically for managed resources).
 * Safe to call even when Aspire is not running — it's a no-op.
 *
 * Heavy SDK packages are dynamically imported only when telemetry is enabled,
 * keeping the main bundle small for production use.
 */
interface TelemetryConfig {
  endpoint: string
  serviceName: string
  serviceVersion: string
  debugDiag: boolean
}

/** Read telemetry configuration from environment variables. Returns null if disabled. */
function readTelemetryConfig(env: Record<string, string | undefined>): TelemetryConfig | null {
  const endpoint = env.OTEL_EXPORTER_OTLP_ENDPOINT
  if (!endpoint) return null
  return {
    endpoint,
    serviceName: env.OTEL_SERVICE_NAME ?? 'buddy',
    serviceVersion: env.npm_package_version ?? '0.0.0',
    debugDiag: env.OTEL_LOG_LEVEL === 'debug',
  }
}

/** Create metric instruments on the shared meter. */
function initMetricHandles(m: Meter): void {
  ipcCallCounter = m.createCounter('buddy.ipc.calls', {
    description: 'Number of IPC handler invocations',
    unit: '{calls}',
  })
  ipcDurationHistogram = m.createHistogram('buddy.ipc.duration', {
    description: 'IPC handler execution time',
    unit: 'ms',
  })
  ipcErrorCounter = m.createCounter('buddy.ipc.errors', {
    description: 'Number of IPC handler errors',
    unit: '{errors}',
  })
  windowOpenCounter = m.createCounter('buddy.windows.opened', {
    description: 'Number of browser windows opened',
    unit: '{windows}',
  })
}

export async function initTelemetry(): Promise<void> {
  if (sdk) return

  const config = readTelemetryConfig(process.env as Record<string, string | undefined>)
  if (!config) {
    console.log('[Telemetry] OTEL_EXPORTER_OTLP_ENDPOINT not set — telemetry disabled')
    return
  }

  if (config.debugDiag) {
    diag.setLogger(new DiagConsoleLogger(), DiagLogLevel.DEBUG)
  }

  try {
    const [
      { NodeSDK },
      { OTLPTraceExporter },
      { OTLPMetricExporter },
      { OTLPLogExporter },
      { PeriodicExportingMetricReader },
      { BatchLogRecordProcessor, LoggerProvider },
      { HttpInstrumentation },
      { DnsInstrumentation },
      { resourceFromAttributes },
    ] = await Promise.all([
      import('@opentelemetry/sdk-node'),
      import('@opentelemetry/exporter-trace-otlp-proto'),
      import('@opentelemetry/exporter-metrics-otlp-proto'),
      import('@opentelemetry/exporter-logs-otlp-proto'),
      import('@opentelemetry/sdk-metrics'),
      import('@opentelemetry/sdk-logs'),
      import('@opentelemetry/instrumentation-http'),
      import('@opentelemetry/instrumentation-dns'),
      import('@opentelemetry/resources'),
    ])

    const resource = resourceFromAttributes({
      'service.name': config.serviceName,
      'service.version': config.serviceVersion,
    })

    const logExporter = new OTLPLogExporter({ url: `${config.endpoint}/v1/logs` })
    const lp = new LoggerProvider({
      resource,
      processors: [new BatchLogRecordProcessor({ exporter: logExporter })],
    })
    logs.setGlobalLoggerProvider(lp)
    loggerProvider = lp

    const nodeSdk = new NodeSDK({
      resource,
      traceExporter: new OTLPTraceExporter({ url: `${config.endpoint}/v1/traces` }),
      metricReaders: [
        new PeriodicExportingMetricReader({
          exporter: new OTLPMetricExporter({ url: `${config.endpoint}/v1/metrics` }),
          exportIntervalMillis: 15_000,
        }),
      ],
      instrumentations: [new HttpInstrumentation(), new DnsInstrumentation()],
    })

    nodeSdk.start()
    sdk = nodeSdk

    initMetricHandles(metrics.getMeter(config.serviceName, config.serviceVersion))

    console.log(
      `[Telemetry] Initialized — exporting to ${config.endpoint} as '${config.serviceName}'`
    )
  } catch (err: unknown) {
    console.warn('[Telemetry] Failed to load SDK packages (non-fatal):', err)
  }
}

/**
 * Gracefully flush and shut down all telemetry providers.
 * Call this in the `before-quit` handler.
 */
export async function shutdownTelemetry(): Promise<void> {
  try {
    await loggerProvider?.shutdown()
    await sdk?.shutdown()
  } catch (err: unknown) {
    console.warn('[Telemetry] Shutdown error:', err)
  }
}

// ---------------------------------------------------------------------------
// Tracing helpers
// ---------------------------------------------------------------------------

const tracer = () => trace.getTracer('buddy')

/**
 * Wrap an async function in an OTel span.
 * Automatically sets status to ERROR on exception and records the error.
 */
export async function withSpan<T>(
  name: string,
  attributes: Record<string, string | number | boolean>,
  fn: (span: Span) => Promise<T>
): Promise<T> {
  return tracer().startActiveSpan(name, { attributes }, async span => {
    try {
      const result = await fn(span)
      span.setStatus({ code: SpanStatusCode.OK })
      return result
    } catch (err: unknown) {
      span.setStatus({ code: SpanStatusCode.ERROR, message: String(err) })
      span.recordException(err instanceof Error ? err : new Error(String(err)))
      throw err
    } finally {
      span.end()
    }
  })
}

// ---------------------------------------------------------------------------
// Metrics helpers
// ---------------------------------------------------------------------------

/** Record an IPC call with its channel name and duration. */
export function recordIpcCall(channel: string, durationMs: number, error?: boolean): void {
  const attrs = { 'ipc.channel': channel }
  ipcCallCounter?.add(1, attrs)
  ipcDurationHistogram?.record(durationMs, attrs)
  if (error) {
    ipcErrorCounter?.add(1, attrs)
  }
}

/** Record a browser window being opened. */
export function recordWindowOpen(target: string): void {
  windowOpenCounter?.add(1, { 'window.target': target })
}

// ---------------------------------------------------------------------------
// Structured logging helper
// ---------------------------------------------------------------------------

const logger = () => logs.getLogger('buddy')

/** Emit a structured log record to the OTLP endpoint. */
export function emitLog(
  severity: 'INFO' | 'WARN' | 'ERROR' | 'DEBUG',
  message: string,
  attributes?: Record<string, string | number | boolean>
): void {
  const severityMap: Record<string, SeverityNumber> = {
    DEBUG: SeverityNumber.DEBUG,
    INFO: SeverityNumber.INFO,
    WARN: SeverityNumber.WARN,
    ERROR: SeverityNumber.ERROR,
  }

  logger().emit({
    severityNumber: severityMap[severity] ?? SeverityNumber.INFO,
    severityText: severity,
    body: message,
    attributes,
  })
}
