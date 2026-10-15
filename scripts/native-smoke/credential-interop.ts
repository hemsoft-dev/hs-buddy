import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, dirname, isAbsolute, join, relative, resolve } from 'node:path'
import { spawnSync } from 'node:child_process'
import {
  createPollenVault,
  credentialAccountForPath,
  type CredentialVault,
} from '../../electron/services/pollenCredentials'

if (process.env.GITHUB_ACTIONS !== 'true' || process.env.BUDDY_CREDENTIAL_FIXTURE !== '1') {
  throw new Error('OS credential interop runs only in disposable GitHub Actions fixtures')
}
const evidence = process.env.BUDDY_CREDENTIAL_EVIDENCE
if (!evidence) throw new Error('Set the owned credential evidence path')
const executable =
  process.env.BUDDY_CREDENTIAL_PROBE ??
  resolve(
    `rust/target/debug/examples/pollen-credential-probe${process.platform === 'win32' ? '.exe' : ''}`
  )
if (!existsSync(executable)) throw new Error('Build the Rust credential probe first')
const profile = mkdtempSync(join(tmpdir(), 'buddy-credential-interop-'))
const config = join(profile, 'config.json')
const keychain = join(profile, 'fixture.keychain-db')
// Public fixture password, never a user credential. This keychain exists only
// on the disposable runner and is deleted after restoring its prior default.
const keychainPassword = 'buddy-ci-fixture-keychain'
let previousKeychain: string | undefined
let fixtureAclDiagnostic: string | undefined
let probeStatus: number | null | undefined
let probeSignal: string | null | undefined
function security(args: string[]): string {
  const result = spawnSync('/usr/bin/security', args, { encoding: 'utf8', timeout: 15_000 })
  if (result.error || result.status !== 0)
    throw new Error('Owned macOS fixture keychain operation failed')
  return result.stdout.trim()
}
function grantFixtureAccess(): void {
  if (process.platform !== 'darwin') return
  const result = spawnSync(
    'python3',
    ['scripts/native-smoke/credential-macos-access.py', config, process.execPath, executable],
    { encoding: 'utf8', timeout: 15_000 }
  )
  if (result.error || result.status !== 0) {
    fixtureAclDiagnostic = result.stderr.trim().slice(0, 1000)
    throw new Error('Owned fixture ACL authorization failed')
  }
}
writeFileSync(join(profile, '.buddy-credential-fixture'), 'buddy credential fixture v1')
writeFileSync(config, JSON.stringify({ ui: { pollenApiKey: '' }, fixtureUnknown: 7 }))

function probe(mode: string, input = ''): string {
  const result = spawnSync(executable, [mode, config], {
    input,
    encoding: 'utf8',
    timeout: 30_000,
    windowsHide: true,
  })
  probeStatus = result.status
  probeSignal = result.signal
  // Never include child output or expected/actual credential values in errors.
  if (result.error || result.status !== 0)
    throw new Error(`Rust credential fixture failed in ${mode}`)
  return result.stdout.trim()
}

function requireMatch(matches: boolean, description: string): void {
  if (!matches) throw new Error(description)
}

let vault: CredentialVault | undefined
let stage = 'create fixture entry'
let failure: Error | undefined
try {
  if (process.platform === 'darwin') {
    previousKeychain = security(['default-keychain', '-d', 'user']).replace(/^"|"$/g, '')
    if (!isAbsolute(previousKeychain))
      throw new Error('Could not identify the runner default keychain')
    security(['create-keychain', '-p', keychainPassword, keychain])
    security(['set-keychain-settings', '-lut', '3600', keychain])
    security(['unlock-keychain', '-p', keychainPassword, keychain])
    security(['default-keychain', '-d', 'user', '-s', keychain])
  }
  vault = createPollenVault(config)
  requireMatch(
    probe('account') === credentialAccountForPath(config),
    'Node and Rust profile identities differ'
  )
  const ascii = 'fixture-only-pollen-ascii'
  stage = 'Node-to-Rust ASCII'
  await vault.write(ascii)
  grantFixtureAccess()
  probe('read', ascii)
  const unicode = 'fixture-only-pollen-é-π-😀'
  stage = 'Rust-to-Node Unicode'
  probe('write', unicode)
  grantFixtureAccess()
  requireMatch((await vault.read()) === unicode, 'Unicode Rust-to-Node password interop failed')
  stage = 'Node deletion'
  await vault.delete()
  probe('read-none')
  const legacy = 'fixture-only-legacy-pollen'
  stage = 'legacy migration'
  writeFileSync(config, JSON.stringify({ ui: { pollenApiKey: legacy }, fixtureUnknown: 7 }))
  probe('read', legacy)
  grantFixtureAccess()
  requireMatch(
    (await vault.read()) === legacy,
    'Legacy migration did not reach the shared OS entry'
  )
  requireMatch(
    !readFileSync(config, 'utf8').includes(legacy),
    'Legacy credential was retained after successful migration'
  )
  requireMatch(
    (JSON.parse(readFileSync(config, 'utf8')) as { fixtureUnknown: number }).fixtureUnknown === 7,
    'Migration lost an unknown configuration field'
  )
  stage = 'clear and stale legacy rejection'
  probe('clear')
  grantFixtureAccess()
  requireMatch((await vault.read()) === '', 'Explicit clear did not retain its safe tombstone')
  writeFileSync(config, JSON.stringify({ ui: { pollenApiKey: legacy }, fixtureUnknown: 7 }))
  probe('read-none')
  requireMatch(
    !readFileSync(config, 'utf8').includes(legacy),
    'A stale legacy snapshot restored the cleared credential'
  )
  stage = 'Rust deletion'
  probe('delete')
  requireMatch(
    (await vault.read()) === null,
    'Rust deletion did not remove the Node-visible fixture entry'
  )
  mkdirSync(dirname(evidence), { recursive: true })
  writeFileSync(
    evidence,
    JSON.stringify(
      {
        result: 'passed',
        platform: process.platform,
        architecture: process.arch,
        macosAuthorization:
          process.platform === 'darwin'
            ? 'explicit access to the disposable fixture entry only; product ACL unchanged'
            : undefined,
        checks: [
          'matching profile identity',
          'Node-to-Rust ASCII',
          'Rust-to-Node Unicode',
          'cross-process delete',
          'legacy migration',
          'unknown-field retention',
          'clear tombstone',
          'stale legacy rejection',
        ],
      },
      null,
      2
    )
  )
  console.log('Credential interop passed; no credential values were logged.')
} catch (_: unknown) {
  mkdirSync(dirname(evidence), { recursive: true })
  writeFileSync(
    evidence,
    JSON.stringify(
      { result: 'failed', stage, probeStatus, probeSignal, fixtureAclDiagnostic },
      null,
      2
    )
  )
  failure = new Error(`Credential interop failed during ${stage}; credential values are withheld.`)
}
try {
  await vault?.delete()
} catch (_: unknown) {
  failure = new Error('Fixture credential cleanup failed; credential values are withheld.')
}
if (previousKeychain) {
  try {
    security(['default-keychain', '-d', 'user', '-s', previousKeychain])
    if (existsSync(keychain)) security(['delete-keychain', keychain])
  } catch (_: unknown) {
    failure = new Error('Owned fixture keychain cleanup failed')
  }
}
const child = relative(resolve(tmpdir()), resolve(profile))
if (
  child.startsWith('..') ||
  isAbsolute(child) ||
  !basename(profile).startsWith('buddy-credential-interop-')
) {
  throw new Error('Refusing cleanup outside the owned fixture directory')
}
rmSync(profile, { recursive: true, force: true })
if (failure) throw failure
