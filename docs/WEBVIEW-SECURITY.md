# Webview Security

Buddy keeps Electron `webviewTag` enabled for the Bookmarks in-app browser.
The feature is user-facing and documented in `README.md` and `docs/VISION.md`.

`webviewTag` is intentionally limited to browser tabs that load public `http`
or `https` URLs in the `persist:browser` partition. It must not be used for
privileged app UI or local/internal resources.

## Guardrails

The main process registers these controls before creating app windows:

- Deny browser webview permission prompts by default.
- Deny `window.open` popups by default.
- Block webview attachment unless the requested partition is `persist:browser`.
- Block webview attachment unless the initial `src` is a public `http` or
  `https` URL accepted by `validateUrl`.
- Strip guest preload settings from webviews.
- Force guest preferences to keep `nodeIntegration` off, `contextIsolation` on,
  `sandbox` on, `webSecurity` on, plugins off, insecure content off, and
  subframe Node integration off.

Navigation to local, loopback, private, and internal hostnames should stay
blocked at the app URL-validation boundary. Future changes that broaden allowed
URLs or enable webview permissions must be treated as security-sensitive.

## Buddy Native

Buddy Native embeds pages on the operating system's engine instead of
`webviewTag`. Its rules are stricter: every navigation is checked against DNS before
it sends a request, and downloads are refused. See
[the native embedded browser](NATIVE-BROWSER.md).

## Verification

Run these checks after changing browser or Electron window behavior:

```powershell
bun run security:electron
bunx vitest run --config vitest.electron.config.ts electron/main.test.ts
bunx vitest run src/components/BrowserTabView.test.tsx
bun run typecheck
```
