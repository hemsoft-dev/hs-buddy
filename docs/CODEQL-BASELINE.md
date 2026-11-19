# Initial CodeQL security review

[Issue #661](https://github.com/HemSoft/hs-buddy/issues/661) covers alerts 1–15
from the first JavaScript/TypeScript analysis. The table records the code
changes and the two intentional flows. GitHub Code Scanning remains the
authority for each alert's current state. No query, production path, or
severity is excluded from analysis.

## Alert decisions

| Alert | Treatment | Evidence and verification |
| --- | --- | --- |
| [1](https://github.com/HemSoft/hs-buddy/security/code-scanning/1) | Encrypt remembered weather location; remove plaintext caches | [Protected location storage](../electron/services/protectedWeatherLocation.ts), [tests](../electron/services/protectedWeatherLocation.test.ts), [config integration test](../electron/config.test.ts), and [renderer memory storage tests](../src/utils/locationSessionStorage.test.ts). Weather and pollen hooks keep coordinates and derived forecasts in memory. |
| [2](https://github.com/HemSoft/hs-buddy/security/code-scanning/2) | Escape every regular-expression metacharacter in the version | Fixed with tests in `scripts/changelog-from-commit.ts`. That script was removed when commits stopped writing CHANGELOG.md, so the flow no longer exists. |
| [3](https://github.com/HemSoft/hs-buddy/security/code-scanning/3) | Decode each original HTML entity once | [Decoder tests](../src/utils/networkSecurity.test.ts) preserve nested entities, decode valid Unicode scalar values, and replace invalid references instead of decoding newly produced ampersands again. |
| [4](https://github.com/HemSoft/hs-buddy/security/code-scanning/4) | Intentional benchmark report, dismissal reason `used in tests` | The data flow and destination are described below. |
| [5](https://github.com/HemSoft/hs-buddy/security/code-scanning/5) | Read notification audio through one bounded file handle | [Handler tests](../electron/ipc/configHandlers.test.ts) cover oversized, unreadable, and non-regular files; the shared reader tests cover growth. |
| [6](https://github.com/HemSoft/hs-buddy/security/code-scanning/6) | Read metrics metadata and bytes from the same opened file | [Metrics handler tests](../electron/ipc/copilotMetricsHandlers.test.ts) cover data, modification time, and read failures. |
| [7](https://github.com/HemSoft/hs-buddy/security/code-scanning/7) | Enforce the file-viewer limit during the read | [File-viewer tests](../electron/ipc/filesystemHandlers.test.ts) and [shared file tests](../electron/services/fileSnapshots.test.ts) cover exact limits, partial UTF-8 reads, growth, path replacement, and handle closure. |
| [8](https://github.com/HemSoft/hs-buddy/security/code-scanning/8) | Open session detail once, then inspect and read that descriptor | [Session tests](../electron/services/copilotSessionService.test.ts) cover missing files, small-file reads, and descriptor closure before large-file or metadata-error streaming fallback. |
| [9](https://github.com/HemSoft/hs-buddy/security/code-scanning/9) | Open the changelog directly and retain its descriptor | Fixed in `scripts/changelog-from-commit.ts`, which has since been removed (see alert 2). |
| [10](https://github.com/HemSoft/hs-buddy/security/code-scanning/10) | Write and truncate through the descriptor used to read the changelog | Fixed in the same removed script (see alert 2). |
| [11](https://github.com/HemSoft/hs-buddy/security/code-scanning/11) | Measure and parse the same bundle byte buffer | [Bundle tests](../scripts/bundle-size-electron.test.ts) exercise UTF-8 byte counts, parsed module imports, comment/string false positives, cyclic imports, and descriptive missing-file errors. |
| [12](https://github.com/HemSoft/hs-buddy/security/code-scanning/12) | Create the e18e placeholder with exclusive `wx` creation | [Placeholder tests](../scripts/e18e-placeholder.test.ts) verify an existing bundle cannot be truncated and unrelated filesystem errors propagate. |
| [13](https://github.com/HemSoft/hs-buddy/security/code-scanning/13) | Create a unique temporary directory for each parsing benchmark run | [Parsing benchmark](../electron/services/copilotSessionParsing.bench.ts) uses `mkdtempSync` and removes only its own generated fixture directory. |
| [14](https://github.com/HemSoft/hs-buddy/security/code-scanning/14) | Create a unique temporary directory for each workspace benchmark run | [Workspace benchmark](../electron/services/copilotSessionService.bench.ts) uses `mkdtempSync`; fixture contents are synthetic. |
| [15](https://github.com/HemSoft/hs-buddy/security/code-scanning/15) | Intentional authenticated usage request, dismissal reason `won't fix` | The fixed destination, selected fields, redirect policy, and regression test are described below. |

## Location privacy boundary

The main process stores only `weatherLocationCiphertext`, produced by
Electron `safeStorage.encryptString`. Base64 transports the encrypted bytes;
base64 itself is not encryption. The renderer receives decrypted coordinates
through the existing weather-location IPC contract and keeps them in memory.
Weather and pollen requests still send the selected coordinates to their
configured service endpoints as required to obtain local forecasts.

At startup, the config manager removes the legacy `ui.weatherLocation`
plaintext value and retains it in memory for encrypted migration. After Electron
is ready, startup encrypts that pending value before opening a window, regardless
of whether the Weather card is enabled. The renderer
also removes the legacy `weather:location`, `weather:cache`, and `pollen:cache`
keys before rendering, including when the weather card is hidden. The hooks
perform the same cleanup when mounted independently. Browser-only legacy
locations without a main-process copy are discarded. Backups and previously
deleted disk blocks are outside this migration's scope.

If encryption is unavailable, fails, or Linux selects `basic_text`, a changed
location remains in memory for the current session and the previous ciphertext
is cleared. There is no plaintext persistence fallback. A later successful
read retries encryption. Existing ciphertext is retained when it cannot be
decrypted, allowing recovery when the OS credential store becomes available.
Corrupt or structurally invalid decrypted locations are ignored.

The protection is against plaintext disclosure from application storage.
It does not protect coordinates from code already running as the logged-in
user or from the application while it is using the coordinates. OS-specific
security properties and availability are documented in
[Electron safeStorage](https://github.com/electron/electron/blob/main/docs/api/safe-storage.md).

## Intentional network and filesystem flows

### Alert 4: local benchmark metadata to a JSON report

The SARIF path starts at the benchmark's
`http://127.0.0.1:<allocated-port>/json/version` response. The benchmark launches
that Electron process with a fresh temporary profile. It extracts the
`Electron/<version>` portion of `User-Agent`, includes it as the
`electronVersion` report field, and serializes the report with `JSON.stringify`.
The version text is data, never executable code or a destination filename.

The only report destinations are the local operator's `--output` and
`--record-baseline` arguments, resolved before the benchmark runs. Neither
the response nor its headers choose the path. The sink is deliberately a
benchmark JSON artifact. This supports the `used in tests` dismissal without
excluding benchmark code from future scans. See the
[runtime](../perf/electron-memory-runtime.ts) and
[report writer](../perf/electron-memory.ts).

### Alert 15: Codex login credentials to the usage service

The [usage service](../electron/services/codexUsageService.ts) reads the local
Codex CLI authentication JSON and selects only its access token and optional
account ID. It sends them as `Authorization: Bearer` and
`ChatGPT-Account-Id` headers to the constant HTTPS endpoint
`https://chatgpt.com/backend-api/wham/usage`. The file contents cannot select
the host, URL, or request body. The request uses `redirect: 'error'`, so a
server redirect cannot introduce a second destination. Credentials are not
returned with the usage result.

This file-to-network flow is required for the user-selected usage feature;
removing it would remove authentication. The `won't fix` dismissal accepts
that intentional flow, not arbitrary file upload. The
[regression test](../electron/services/codexUsageService.test.ts) asserts the
exact endpoint, selected headers, rejected redirects, and credential-free
result with synthetic credentials.

## Filesystem boundaries

File-handle operations prevent a pathname replacement between a check and
a later open from selecting a different file. The shared reader checks file
type and reads at most the caller's byte limit plus one, including when the
opened file grows. Its buffer starts at the reported file size plus one and
grows only as needed, up to that limit. It always closes the handle. This does not lock out
concurrent writers to the same inode or make metadata and content an atomic
snapshot. The session parser's small-file cutoff selects its parsing path;
large sessions retain the existing streaming path.

The changelog writer uses explicit descriptor positions and truncates after
writing. It does not claim crash-atomic replacement. The e18e helper owns its
exclusive placeholder during analysis and removes it afterward. Builds and
e18e analysis must remain sequential in a given checkout, as in CI; the
helper is not a concurrent-build coordinator.

The benchmark directories are created by `mkdtempSync`, not a predictable
pathname assembled by the caller. They inherit the operating system's
temporary-directory access rules and contain synthetic fixture data.
See [Node filesystem documentation](https://github.com/nodejs/node/blob/main/doc/api/fs.md)
for descriptor reads, exclusive creation, and temporary directories.
