import { rm } from 'node:fs/promises'

export async function removeOwnedProfile(
  profilePath: string,
  remove: typeof rm = rm
): Promise<void> {
  // Bun 1.4.2 ignores fs.rm's maxRetries option. Keep these retries explicit
  // so both supported harness runtimes allow a closed process's lock to drain.
  for (let attempt = 0; ; attempt++) {
    try {
      await remove(profilePath, { recursive: true, force: true })
      return
    } catch (error: unknown) {
      const code =
        typeof error === 'object' && error !== null && 'code' in error ? error.code : null
      if (
        attempt === 3 ||
        typeof code !== 'string' ||
        !['EBUSY', 'EMFILE', 'ENFILE', 'ENOTEMPTY', 'EPERM'].includes(code)
      )
        throw error
      await new Promise(resolve => setTimeout(resolve, 500 * (attempt + 1)))
    }
  }
}

export async function withProfileCleanup<T>(
  profilePath: string,
  action: () => Promise<T>,
  close: () => Promise<void>
): Promise<T> {
  let outcome: { ok: true; value: T } | { ok: false; error: unknown }
  try {
    outcome = { ok: true, value: await action() }
  } catch (error: unknown) {
    outcome = { ok: false, error }
  }
  try {
    await close()
    await removeOwnedProfile(profilePath)
  } catch (error: unknown) {
    if (outcome.ok) throw error
    console.warn('Memory-profile cleanup also failed. The original measurement error is retained.')
  }
  if (!outcome.ok) throw outcome.error
  return outcome.value
}
