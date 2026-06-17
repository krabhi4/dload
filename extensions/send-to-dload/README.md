# Send to dload — browser extension

A Manifest V3 browser extension that detects **any** downloadable artifact your browser encounters — HTTP(S)/FTP file downloads, magnet links, `.torrent` file URLs — and forwards them to your self-hosted [dload](https://github.com/krabhi4/dload) server.

Targets **Chromium** (Chrome / Edge / Brave / Opera) and **Firefox** from a single source tree.

> Status: **0.1.0 alpha**. No store listing yet — install manually from the GitHub Release artifacts.

---

## Features

- **Three detection channels** that work together:
  1. **Browser download interception** — any HTTP(S)/FTP file the browser tries to save is cancelled and reposted to dload (if dload *permanently* rejects it the browser download is re-issued; transient outages are queued for retry).
  2. **Right-click "Send to dload"** — works on magnet links, `.torrent` URLs, and video/audio links.
  3. **Zero-click magnet scanner** — a content script intercepts left-clicks on `a[href^="magnet:"]` and `a[href$=".torrent"]` on every page.
- **Durable retry queue** — failed sends are persisted to `chrome.storage.local` and retried with exponential backoff (30s → 6h, max 10 attempts) so transient outages don't lose downloads.
- **Filter pipeline** — per-host whitelist/blacklist, extension include/exclude lists, size floor/ceiling, per-kind toggles, master enable switch.
- **Activity log** — last 200 detection events viewable in the Options page; clearable.
- **Cross-browser** — single source tree, single manifest, single test suite. Chrome uses service-worker background; Firefox uses event-page. Both run the same `src/background/index.js` code.
- **Local-only API key** — your dload API key never leaves the extension except for direct POSTs to your configured dload server.

---

## Install

### Chrome / Edge / Brave / Opera

1. Download `send-to-dload-0.1.0.zip` from the [latest release](https://github.com/krabhi4/dload/releases).
2. Unzip it.
3. Visit `chrome://extensions` (or `edge://extensions`, `brave://extensions`, `opera://extensions`).
4. Turn on **Developer mode** (top right).
5. Click **Load unpacked** → select the unzipped folder.

### Firefox

1. Download `send-to-dload-0.1.0.xpi` from the [latest release](https://github.com/krabhi4/dload/releases).
2. Visit `about:addons`.
3. Click the gear icon → **Install Add-on From File** → pick the `.xpi`.
4. Confirm the permission prompt.

> For development: `npm run dev:ff` (Firefox) or `npm run dev:cr` (Chromium) auto-loads `src/` with hot-reload via `web-ext run`.

---

## Configure

1. Click the toolbar icon → **Settings** (or right-click the toolbar icon → **Options**).
2. Enter your **dload server URL** (e.g. `http://192.168.0.10:8080`).
3. Paste an **API key** — generate one in the dload web UI under **Profile → API keys** (it's shown once; copy it then).
4. Click **Test connection**. You should see "Connected as &lt;username&gt; (&lt;role&gt;)".
5. Adjust the filter rules to taste (defaults are sensible).
6. **Save settings**.

### CORS

You normally do **not** need to configure CORS. The extension talks to dload from
its background service worker using its host permissions (`http://*/*`,
`https://*/*`), and browsers exempt extension host-permission requests from CORS,
so no `Access-Control-Allow-Origin` header and no preflight are required. This was
verified end-to-end against a stock dload server with `DLOAD_CORS_ORIGIN` unset.

If you restrict the extension's site access (e.g. Chrome's "on click" mode) so it
no longer covers your dload origin, re-grant access from the extension's
site-access settings. That, not a server env var, is what gates the request.

---

## Filter rules

| Setting | Effect |
|---|---|
| **Extension enabled** | Master kill-switch. |
| **Host filter mode** | `Blacklist` captures everything except listed hosts. `Whitelist` captures only listed hosts. Hostnames support `*.example.com` for subdomain matching. |
| **Allowed extensions** | Comma- or newline-separated. Captures matching URLs only. Empty = no include filter. |
| **Excluded extensions** | Skips matching URLs even if in the include list. |
| **Min / Max size** | Skips downloads outside the size range. Skipped when size unknown. |
| **Capture magnet links** | Per-kind toggle. |
| **Capture HTTP/HTTPS/FTP** | Per-kind toggle. |
| **Notification on send / skip** | Optional desktop notifications. |

Defaults: blacklist mode, empty host list, include list `[.torrent, .iso, .zip, .rar, .7z, .tar, .gz, .bz2, .xz, .dmg, .apk, .exe, .msi, .deb, .rpm, .pkg, .mp4, .mkv, .avi, .mov, .webm, .m4v, .mp3, .flac, .ogg, .m4a, .opus, .wav, .pdf, .epub, .mobi]`, exclude `[.html, .htm, .js, .mjs, .css, .png, .jpg, .jpeg, .gif, .svg, .webp, .avif, .ico, .txt, .json, .xml]`.

---

## Detection flow (high-level)

```
                              ┌──────────────────────────┐
                              │  browser.onDetermining   │  channel 1
                              │  Filename (cancelled)    │
                              └─────────────┬────────────┘
                                            │
                                              ┌────────────┐
   ┌─────────────────────────┐               │            │
   │ contextMenus.onClicked  │ channel 2     │            │
   │  → onCandidate(url)     │ ───────────►  │            │
   └─────────────────────────┘               ▼            ▼
                                ┌──────────────────────────┐
   ┌─────────────────────────┐   │ filter pipeline          │
   │ content-script magnet    │   │  classify → shouldCapture│
   │ click interceptor       │   │  → applyHostFilter       │
   │ → onCandidate(url)       │ ──►                          │
   └─────────────────────────┘   └──────────┬───────────────┘
                                              │ capture=true
                                              ▼
                                  POST /api/downloads
                                              │
                              ┌───────────────┴────────────────┐
                              │                                │
                       success                          failure
                              │                                │
                  notification + log                retry queue (1m alarm)
                                                             │
                                                  re-POST with backoff
                                                  30s → 1m → 5m → 15m
                                                  → 1h → 6h (×4), max 10
```

---

## Permissions requested

| Permission | Why |
|---|---|
| `contextMenus` | Right-click "Send to dload" entry. |
| `downloads` | Intercept browser downloads via `onDeterminingFilename`. |
| `storage` | Save config + retry queue. |
| `alarms` | Survive service-worker shutdown — drives the retry queue. |
| `notifications` | Desktop feedback on send / skip / error. |
| `activeTab` | Read the current tab's URL when the popup button is clicked. |
| `http://*/*` + `https://*/*` (host permissions) | The content-script magnet scanner runs on every page; also lets the background `fetch()` talk to user-configured dload servers. |

---

## Troubleshooting

### "Unauthorized"
The API key is invalid or was revoked. Generate a new one in the dload web UI (Profile → API keys) and paste it into the Options page.

### `NetworkError` when attempting a fetch
- Confirm the dload server URL is reachable from the browser (open it in a tab).
- Confirm the extension still has site access to the dload origin (Chrome: chrome://extensions → Details → "Site access"). Extension host permissions, not a server CORS env var, are what allow the request.

### "Extension loaded but nothing happens"
- Open Options → **Recent activity**. If you see nothing at all, the content script isn't matching your links — make sure it's a `magnet:…` URL or a `.torrent` link or an allowed-extension HTTP URL.
- If you see `skipped: extension-not-included`, your URL's extension isn't in the include list — add it.
- If you see `skipped: not-configured`, the server URL or token isn't set.

### "Downloaded twice — once by the browser, once by dload"
The extension cancels the browser download before reposting to dload. If dload *permanently* rejects the request (e.g. a bad token, or a URL dload refuses), the browser download is re-issued so you don't lose the file — that's the one case both sides run. A *transient* failure (server briefly unreachable) is instead queued and retried against dload only, so it won't produce a browser copy. Fix the underlying error in dload to stop seeing duplicates.

### Token storage
The API key lives in `chrome.storage.local`, which is plaintext on disk inside the browser profile directory. Treat your browser profile like a password manager, and revoke the key in dload if the profile is compromised.

---

## Known lint warnings (intentional)

`web-ext lint` emits three warnings against this extension that are **expected** for a cross-browser MV3 extension and do not fail the lint (exit code 0):

| Warning | Reason |
|---|---|
| `/background/service_worker is not supported` | Firefox ignores the field (it uses `background.scripts` instead); Chrome requires it. Declaring both is the canonical cross-browser idiom. |
| `downloads.onDeterminingFilename is not supported` (×2) | Firefox does not implement `chrome.downloads.onDeterminingFilename` yet. The code feature-checks the API (`browser.downloads?.onDeterminingFilename`) and falls back to `downloads.onCreated`, but addons-linter's static analyzer cannot follow the optional-chain and warns anyway. |

All three are documented in code; they are not bugs.

---

## Development

```bash
cd extensions/send-to-dload
npm ci                 # install dev deps (web-ext, vitest, eslint)
npm test               # vitest unit tests
npm run lint           # web-ext lint + eslint
npm run dev:ff         # Firefox with hot-reload
npm run dev:cr         # Chromium with hot-reload
npm run build          # produce dist/chrome/*.zip + dist/firefox/*.xpi
```

### Project layout

```
extensions/send-to-dload/
├── manifest.json
├── package.json
├── package-lock.json
├── eslint.config.js
├── vitest.config.js
├── web-ext-config.mjs
├── _locales/en/messages.json
├── scripts/
│   └── build.mjs
├── src/
│   ├── shared/
│   │   ├── polyfill.js        # browser.* Promise shim for Chrome
│   │   ├── classify.js        # URL → kind + host matcher
│   │   ├── config.js          # chrome.storage.local schema + accessors
│   │   ├── lock.js            # serializes storage read-modify-write
│   │   └── logger.js
│   ├── background/
│   │   ├── index.js           # service-worker / event-page entry
│   │   ├── api.js             # verifyToken + sendToDload
│   │   ├── detector.js        # 3-channel funnel
│   │   ├── filters.js         # shouldCapture + applyHostFilter
│   │   ├── retry.js           # persistent retry queue + alarm driver
│   │   └── messaging.js       # typed message names
│   ├── content/
│   │   └── content.js         # magnet/.torrent DOM scanner
│   ├── popup/                 # toolbar popup
│   ├── options/               # settings page
│   └── icons/                 # 16/48/128 PNG + SVG
└── tests/unit/
    ├── api.test.mjs
    ├── classify.test.mjs
    ├── filters.test.mjs
    ├── retry.test.mjs
    └── config.test.mjs
```

### Releasing

1. Bump `version` in `manifest.json`.
2. `npm run lint && npm test && npm run build`.
3. `gh release create vX.Y.Z dist/chrome/*.zip dist/firefox/*.xpi --generate-notes`.
4. Edit the auto-generated notes to match the install + CORS sections above.

---

## Limitations (v0.1.0)

- **Folder override** — the extension posts JSON (`{url, folder_id}`) and dload honors `folder_id`, but there is no Options field to pick a default folder yet, so everything lands in dload's default folder for now.
- **Filename override** — the extension sends the suggested filename, but the dload server currently ignores it and derives the name from the URL.
- **No Chrome Web Store / AMO** listing for v0.1.
- **No telemetry.**
- **No per-tab "send everything on this page" mode.**

---

## License

GPL-3.0-only — same as the [dload server](https://github.com/krabhi4/dload).