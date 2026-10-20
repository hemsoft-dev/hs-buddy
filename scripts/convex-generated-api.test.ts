import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { expect, it } from 'vitest'

// `convex dev` rewrites convex/_generated when a module is missing from it. In
// native qualification that dirtied the checkout and failed the clean-source
// gate (#853), so the committed API must already list every module.
function modules(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      return ['_generated', '__tests__'].includes(entry.name) ? [] : modules(path)
    }
    // Convex skips the schema, the app config and files with extra dots.
    const name = relative('convex', path).replaceAll('\\', '/')
    if (!/^[^.]+\.ts$/.test(entry.name) || ['schema.ts', 'convex.config.ts'].includes(name)) {
      return []
    }
    return [name.slice(0, -'.ts'.length)]
  })
}

it('lists every Convex module in the committed generated API', () => {
  const api = readFileSync('convex/_generated/api.d.ts', 'utf8')
  const listed = new Set(
    [...api.matchAll(/^ {2}(?:"([^"]+)"|([A-Za-z]\w*)): typeof /gm)].map(
      match => match[1] ?? match[2]
    )
  )
  const present = modules('convex')
  expect(present.length).toBeGreaterThan(20)
  expect(present.filter(module => !listed.has(module))).toEqual([])
  expect([...listed].filter(module => !present.includes(module))).toEqual([])
})
