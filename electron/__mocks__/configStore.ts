export function createConfigStoreMock(fixture: { initial: Record<string, unknown> }) {
  return class MockStore {
    private data: Record<string, unknown> = {}
    private listeners = new Map<string, Set<() => void>>()
    writes: string[] = []
    path = '/mock/config.json'
    store = {}

    constructor() {
      this.data = structuredClone(fixture.initial)
      this.store = this.data
    }

    get(key: string, defaultValue?: unknown): unknown {
      let value: unknown = this.data
      for (const segment of key.split('.')) {
        if (!value || typeof value !== 'object' || !(segment in value)) return defaultValue
        value = (value as Record<string, unknown>)[segment]
      }
      return value
    }

    set(key: string, value: unknown): void {
      const previous = new Map(
        [...this.listeners.keys()].map(listenerKey => [
          listenerKey,
          JSON.stringify(this.get(listenerKey)),
        ])
      )
      const segments = key.split('.')
      let target = this.data
      for (const segment of segments.slice(0, -1)) {
        const child = target[segment]
        if (!child || typeof child !== 'object') target[segment] = {}
        target = target[segment] as Record<string, unknown>
      }
      target[segments.at(-1)!] = value
      this.writes.push(key)
      for (const [listenerKey, listeners] of this.listeners) {
        if (previous.get(listenerKey) === JSON.stringify(this.get(listenerKey))) continue
        for (const listener of listeners) listener()
      }
    }

    onDidChange(key: string, listener: () => void): () => void {
      const listeners = this.listeners.get(key) ?? new Set<() => void>()
      listeners.add(listener)
      this.listeners.set(key, listeners)
      return () => listeners.delete(listener)
    }

    has(key: string): boolean {
      return key in this.data
    }

    delete(key: string): void {
      delete this.data[key]
    }

    clear(): void {
      this.data = {}
      this.store = this.data
      this.writes = []
    }
  }
}
