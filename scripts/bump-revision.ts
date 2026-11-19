/**
 * Increment the PATCH (revision) version to cut a release.
 *
 * Run it in a pull request when the next merge to main should be published:
 * the release workflow publishes a package.json version that differs from its
 * parent commit, with notes generated from the merged pull request titles.
 * The renderer (src/constants/appVersion.ts) and the native build
 * (rust/crates/buddy-app/build.rs) both read the version from package.json.
 *
 * Usage: bun run bump
 */

import { readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

const ROOT = resolve(import.meta.dirname, '..')

// --- 1. Bump package.json ---
const pkgPath = resolve(ROOT, 'package.json')
const pkg = JSON.parse(readFileSync(pkgPath, 'utf-8'))
const [major, minor, patch] = pkg.version.split('.').map(Number)
const newVersion = `${major}.${minor}.${patch + 1}`
pkg.version = newVersion
writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + '\n', 'utf-8')
console.log(`✓ package.json  ${major}.${minor}.${patch} → ${newVersion}`)

console.log(`\n🏷️  Revision bumped to ${newVersion}`)
