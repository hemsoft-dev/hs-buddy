import { describe, expect, it } from 'vitest'
import { viewLabels } from '../src/components/appContentViewLabels'
import fixture from './matching-fixture.json'

describe('matching performance fixture', () => {
  it('requires every Electron destination to match its selected-tab receipt', () => {
    for (const route of fixture.routes) {
      expect(viewLabels[route]).toBe(
        fixture.electronLabels[route as keyof typeof fixture.electronLabels]
      )
    }
  })
})
