import { describe, it, expect, vi, beforeEach } from 'vitest'

vi.mock('electron', () => ({
  ipcMain: { handle: vi.fn() },
  net: { fetch: vi.fn() },
}))

vi.mock('../config', () => ({
  configManager: {
    getPollenApiKey: vi.fn(),
  },
}))

import { ipcMain, net } from 'electron'
import { registerPollenHandlers } from './pollenHandlers'
import { configManager } from '../config'
import { IPC_INVOKE } from '../../src/ipc/contracts'

const mockConfigManager = vi.mocked(configManager)
const mockNetFetch = vi.mocked(net.fetch)

describe('pollenHandlers', () => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let handlers: Map<string, (...args: any[]) => any>

  beforeEach(() => {
    vi.clearAllMocks()
    handlers = new Map()
    vi.mocked(ipcMain.handle).mockImplementation((channel, handler) => {
      handlers.set(channel, handler)
    })
    registerPollenHandlers()
  })

  it('registers the pollen fetch handler', () => {
    expect(handlers.has(IPC_INVOKE.POLLEN_FETCH_CURRENT)).toBe(true)
  })

  describe('pollen:fetch-current', () => {
    const event = {} as Electron.IpcMainInvokeEvent
    const location = { latitude: 35.82, longitude: -78.82 }

    function invoke(loc: { latitude: number; longitude: number }) {
      return handlers.get(IPC_INVOKE.POLLEN_FETCH_CURRENT)!(event, loc)
    }

    it('returns no-api-key error when API key is missing', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('')

      const result = await invoke(location)

      expect(result).toEqual({ success: false, error: 'no-api-key' })
    })

    it('returns no-api-key error when API key is undefined', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue(undefined as unknown as string)

      const result = await invoke(location)

      expect(result).toEqual({ success: false, error: 'no-api-key' })
    })

    it('reads the API key only once per fetch (no race condition)', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('valid-key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              pollenTypeInfo: [{ code: 'TREE', indexInfo: { value: 1 } }],
              plantInfo: [],
            },
          ],
        }),
      } as Response)

      await invoke(location)

      expect(mockConfigManager.getPollenApiKey).toHaveBeenCalledTimes(1)
    })

    it('returns invalid location error for non-finite latitude', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('test-key')

      const result = await invoke({ latitude: NaN, longitude: -78 })

      expect(result).toEqual({ success: false, error: 'Invalid location' })
    })

    it('returns invalid location error for non-finite longitude', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('test-key')

      const result = await invoke({ latitude: 35, longitude: Infinity })

      expect(result).toEqual({ success: false, error: 'Invalid location' })
    })

    it('fetches from Google Pollen API with correct URL', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('my-api-key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              pollenTypeInfo: [{ code: 'TREE', indexInfo: { value: 3, category: 'Medium' } }],
              plantInfo: [
                {
                  code: 'OAK',
                  displayName: 'Oak',
                  indexInfo: { value: 4, category: 'High' },
                  inSeason: true,
                  plantDescription: { type: 'TREE' },
                },
              ],
            },
          ],
        }),
      } as Response)

      const result = await invoke(location)

      expect(mockNetFetch).toHaveBeenCalledWith(
        expect.stringContaining('pollen.googleapis.com'),
        expect.objectContaining({ headers: { 'User-Agent': 'hs-buddy/1.0' } })
      )
      expect(result.success).toBe(true)
      expect(result.data.tree).toBe(3)
      expect(result.data.species).toHaveLength(1)
      expect(result.data.species[0].code).toBe('OAK')
    })

    it('parses grass and weed pollen types', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              pollenTypeInfo: [
                { code: 'GRASS', indexInfo: { value: 2 } },
                { code: 'WEED', indexInfo: { value: 1 } },
              ],
              plantInfo: [],
            },
          ],
        }),
      } as Response)

      const result = await invoke(location)

      expect(result.success).toBe(true)
      expect(result.data.grass).toBe(2)
      expect(result.data.weed).toBe(1)
    })

    it('collects health recommendations from pollen types', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              pollenTypeInfo: [
                {
                  code: 'TREE',
                  indexInfo: { value: 4 },
                  healthRecommendations: ['Stay indoors', 'Wear a mask'],
                },
              ],
              plantInfo: [],
            },
          ],
        }),
      } as Response)

      const result = await invoke(location)

      expect(result.data.healthRecommendations).toEqual(['Stay indoors', 'Wear a mask'])
    })

    it('returns error when API responds with non-ok status', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: false,
        status: 403,
        json: async () => ({ error: { message: 'API key invalid' } }),
      } as unknown as Response)

      const result = await invoke(location)

      expect(result).toEqual({
        success: false,
        error: 'Google Pollen API: HTTP 403. Check API enablement, key restrictions, and quota.',
      })
    })

    it('falls back to HTTP status when error body has no message', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: false,
        status: 500,
        json: async () => ({}),
      } as unknown as Response)

      const result = await invoke(location)

      expect(result).toEqual({
        success: false,
        error: 'Google Pollen API: HTTP 500. Check API enablement, key restrictions, and quota.',
      })
    })

    it('falls back to HTTP status when error body is unparseable', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: false,
        status: 502,
        json: async () => {
          throw new Error('not JSON')
        },
      } as unknown as Response)

      const result = await invoke(location)

      expect(result).toEqual({
        success: false,
        error: 'Google Pollen API: HTTP 502. Check API enablement, key restrictions, and quota.',
      })
    })

    it('returns error when response has no dailyInfo', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({}),
      } as Response)

      const result = await invoke(location)

      expect(result).toEqual({
        success: false,
        error: 'No pollen data available for this location',
      })
    })

    it('returns error when dailyInfo has empty arrays', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [{ pollenTypeInfo: [], plantInfo: [] }],
        }),
      } as Response)

      const result = await invoke(location)

      expect(result).toEqual({
        success: false,
        error: 'No pollen data available for this location',
      })
    })

    it('handles network errors gracefully', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockRejectedValue(new Error('Network timeout'))

      const result = await invoke(location)

      expect(result).toEqual({ success: false, error: 'Pollen fetch failed' })
    })

    it('withholds reflected credentials in provider and network errors', async () => {
      const secret = 'fixture-key-should-never-reach-renderer'
      mockConfigManager.getPollenApiKey.mockResolvedValue(secret)
      const body = vi.fn().mockResolvedValue({ error: { message: `Invalid key ${secret}` } })
      mockNetFetch.mockResolvedValue({ ok: false, status: 403, json: body } as unknown as Response)
      const rejected = await invoke(location)
      expect(rejected).toEqual({
        success: false,
        error: 'Google Pollen API: HTTP 403. Check API enablement, key restrictions, and quota.',
      })
      expect(body).not.toHaveBeenCalled()
      mockNetFetch.mockRejectedValue(
        new Error(`Request failed: https://example.invalid/?key=${secret}`)
      )
      expect(await invoke(location)).toEqual({ success: false, error: 'Pollen fetch failed' })
    })

    it('skips plants without code or displayName', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              pollenTypeInfo: [{ code: 'TREE', indexInfo: { value: 1 } }],
              plantInfo: [
                { code: 'OAK' }, // missing displayName
                { displayName: 'Birch' }, // missing code
                {
                  code: 'ELM',
                  displayName: 'Elm',
                  indexInfo: { value: 2, category: 'Low' },
                  inSeason: true,
                  plantDescription: { type: 'tree' },
                },
              ],
            },
          ],
        }),
      } as Response)

      const result = await invoke(location)

      expect(result.data.species).toHaveLength(1)
      expect(result.data.species[0].code).toBe('ELM')
      expect(result.data.species[0].type).toBe('TREE') // normalized uppercase
    })

    it('normalizes unknown plant types to TREE', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              pollenTypeInfo: [{ code: 'TREE', indexInfo: { value: 1 } }],
              plantInfo: [
                { code: 'X', displayName: 'Unknown', plantDescription: { type: 'SHRUB' } },
              ],
            },
          ],
        }),
      } as Response)

      const result = await invoke(location)

      expect(result.data.species[0].type).toBe('TREE')
    })

    it('defaults plant fields when optional data is missing', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              pollenTypeInfo: [{ code: 'GRASS', indexInfo: { value: 1 } }],
              plantInfo: [{ code: 'RYE', displayName: 'Rye' }],
            },
          ],
        }),
      } as Response)

      const result = await invoke(location)

      const species = result.data.species[0]
      expect(species.index).toBe(0)
      expect(species.category).toBe('None')
      expect(species.inSeason).toBe(false)
      expect(species.type).toBe('TREE') // no plantDescription → defaults
    })

    it('ignores unknown pollen type codes', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              pollenTypeInfo: [
                { code: 'UNKNOWN', indexInfo: { value: 5 } },
                { code: 'TREE', indexInfo: { value: 2 } },
              ],
              plantInfo: [],
            },
          ],
        }),
      } as Response)

      const result = await invoke(location)

      expect(result.data.tree).toBe(2)
      expect(result.data.grass).toBe(0)
      expect(result.data.weed).toBe(0)
    })

    it('handles pollenTypeInfo with missing code gracefully', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              pollenTypeInfo: [{ indexInfo: { value: 3 } }],
              plantInfo: [{ code: 'A', displayName: 'Alder' }],
            },
          ],
        }),
      } as Response)

      const result = await invoke(location)

      expect(result.success).toBe(true)
      expect(result.data.tree).toBe(0)
    })

    it('defaults indexInfo.value to 0 when value is undefined', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              pollenTypeInfo: [{ code: 'TREE', indexInfo: {} }],
              plantInfo: [],
            },
          ],
        }),
      } as Response)

      const result = await invoke(location)

      expect(result.data.tree).toBe(0)
    })

    it('handles undefined pollenTypeInfo and plantInfo arrays', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              pollenTypeInfo: [{ code: 'GRASS', indexInfo: { value: 1 } }],
              // plantInfo is undefined
            },
          ],
        }),
      } as Response)

      const result = await invoke(location)

      expect(result.success).toBe(true)
      expect(result.data.grass).toBe(1)
      expect(result.data.species).toEqual([])
    })

    it('handles undefined pollenTypeInfo with defined plantInfo', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('key')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              // pollenTypeInfo is undefined
              plantInfo: [
                {
                  code: 'B',
                  displayName: 'Birch',
                  inSeason: true,
                  plantDescription: { type: 'TREE' },
                },
              ],
            },
          ],
        }),
      } as Response)

      const result = await invoke(location)

      expect(result.success).toBe(true)
      expect(result.data.species).toHaveLength(1)
      expect(result.data.species[0].code).toBe('B')
    })
  })

  describe('getTrimmedPollenApiKey edge cases', () => {
    const event = {} as Electron.IpcMainInvokeEvent
    const location = { latitude: 35.82, longitude: -78.82 }

    function invoke(loc: { latitude: number; longitude: number }) {
      return handlers.get(IPC_INVOKE.POLLEN_FETCH_CURRENT)!(event, loc)
    }

    it('returns no-api-key when value is a non-string type (number)', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue(42 as unknown as string)

      const result = await invoke(location)

      expect(result).toEqual({ success: false, error: 'no-api-key' })
    })

    it('returns no-api-key when value is whitespace-only', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('   \t  ')

      const result = await invoke(location)

      expect(result).toEqual({ success: false, error: 'no-api-key' })
    })

    it('trims surrounding whitespace from a valid key', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValue('  my-key  ')
      mockNetFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          dailyInfo: [
            {
              pollenTypeInfo: [{ code: 'TREE', indexInfo: { value: 1 } }],
              plantInfo: [],
            },
          ],
        }),
      } as Response)

      const result = await invoke(location)

      expect(result.success).toBe(true)
      expect(mockNetFetch).toHaveBeenCalledWith(
        expect.stringContaining('key=my-key'),
        expect.anything()
      )
    })
  })
})
