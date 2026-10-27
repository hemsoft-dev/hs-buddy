# Native embedded browser

Bookmarks in Buddy Native open as tabs inside the Bookmarks section, beside the
bookmark list. Each tab shows the page on the operating system's own web engine:
WebView2 on Windows, WKWebView on macOS, and WebKitGTK on Linux under X11.

## Tabs

The tab strip starts with Bookmarks (the list) followed by one tab per open page.
A tab's toolbar has Back, Forward, Reload, the page address, zoom out, the zoom
level, zoom in, and Open in your browser. Closing a tab selects its neighbor;
closing the last one returns to the list. Up to 20 tabs can be open, as many as a
restart restores; opening another asks you to close one first.

Open tabs, their titles, the selected tab (or the list) and the zoom level are saved in the
configuration (`native.browser`) and restored at the next start. A tab follows its page's
address, including route changes a page makes without loading a new document
(`history.pushState`, `#` links); Buddy reads it once a second. A restored tab
starts its page only when it is first shown. Cookies and site storage live in the
app's own profile, `browser-profile` beside the configuration file, so an isolated
`BUDDY_CONFIG_PATH` run never touches the real one. Electron's
`persist:browser` cookies don't carry over: the engines differ.

The tabs are buttons, so Tab reaches them and Enter or Space selects one. As in
Electron, Ctrl+Tab and Ctrl+Shift+Tab move to the next or previous tab (the
bookmark list included) and Ctrl+F4 closes the selected page. These shortcuts work
while Buddy has the keyboard; clicking into a page gives the page the keyboard
until you click Buddy again.

## What a page may do

The rules are stricter than Electron's embedded tab and match its in-app browser
window. They live in `rust/crates/buddy-core/src/browser.rs`:

- Only `http` and `https` pages load. `file:`, `data:`, `javascript:`, custom
  schemes and `about:` pages other than `blank`/`srcdoc` are refused. A `mailto:`
  link is offered to the mail app with a button; it never opens by itself.
- Local and private addresses are refused: `localhost`, `.local`, `.internal`,
  `.home.arpa`, single-label names, loopback, private, link-local and carrier-grade
  NAT ranges (which includes Tailscale), multicast, and IPv4 written as IPv6.
- Every navigation, in the page or in a frame, including a link click or a
  redirect, is decided before the engine sends any request: the scheme and host
  check, then a DNS lookup. If any address the name resolves to is internal, the
  navigation is refused and the tab says why, so a public name pointing at the
  local network can't reach it. The engines ask synchronously (and WebKit doesn't
  say which frame is navigating), so the lookup runs right there, given up after
  3 seconds and remembered for 60 seconds per host. A bookmark or restored tab is
  looked up in the background before it loads, so its own navigation finds the
  answer waiting.
- Popups, downloads and every permission request (camera, microphone, location,
  notifications and the rest) are refused, and the tab says what was refused.
  Downloads have no Electron equivalent; Open in your browser is the way to
  download.
- Pages get no bridge to the app: no IPC handler, no injected scripts, no
  developer tools, no clipboard reads, and (on Windows) no browser accelerator
  keys or default context menus. Credentials stay in the app process; the engines
  run their pages in their own sandboxed processes.

## Platform notes

- **Windows.** GPUI draws through DirectComposition, whose layer covers child
  windows, so Buddy Native starts GPUI without it
  (`GPUI_DISABLE_DIRECT_COMPOSITION=1`, unless already set). Pages need the
  Microsoft Edge WebView2 Runtime, which Windows 11 includes. Without it a tab
  explains that and offers the system browser.
- **macOS.** The page draws above the GPUI content. Website data is kept in a
  persistent store keyed from the profile folder.
- **Linux.** Pages are embedded only under X11. Buddy initializes GTK itself and
  drives its event loop from the app. On Wayland a tab explains that pages can't
  be embedded yet and offers the system browser. The `.deb` depends on
  `libwebkit2gtk-4.1-0`, `libjavascriptcoregtk-4.1-0`, `libsoup-3.0-0` and
  `libgtk-3-0t64`; building needs `libwebkit2gtk-4.1-dev` and `libgtk-3-dev`.

A page is a native view above the window, so Buddy hides it whenever something
drawn by Buddy would sit under it: another tab, another section, an open menu or a
dialog. Toast notifications can be covered while a page is shown.

## Qualification

For installed-app runs, `BUDDY_BROWSER_TEST_ORIGIN=http://127.0.0.1:<port>/` admits
exactly that loopback origin, and `BUDDY_BROWSER_AUDIT_FILE=<path>` appends one
JSON line per refusal (kind, origin only, reason). Both are honored only when
`BUDDY_CONFIG_PATH` is set.

From `rust/`:

```bash
cargo test -p buddy-core browser
cargo test -p buddy-app browser_view
```

The view tests use a fake engine and resolver and check page start, visibility
across tabs, sections, menus and dialogs, the DNS refusals before a load, the tab
shortcuts, the engine pump stopping with the last page, the no-engine fallback,
session restore (the list selection included) and placement on resize.
