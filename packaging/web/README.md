# Hosting SoundCraft for the web

`soundcraft-web-<version>.zip` (from the GitHub release, or `packaging/web/package.sh`) holds a
static site in `soundcraft-web-<version>/`:

| File | What it is |
|---|---|
| `index.html` | The page. It loads everything through relative URLs. |
| `soundcraft-web-<hash>.js` | wasm-bindgen glue (generated, ES module) |
| `soundcraft-web-<hash>_bg.wasm` | The app (size: see the release; roughly a third of that with compression) |
| `_headers`, `.htaccess` | Sample header rules for Netlify/Cloudflare Pages and Apache |

There is no server-side code. Upload the folder's contents anywhere that serves static files.

## Any path works

All URLs in `index.html` are relative (`public_url = "./"` in `apps/soundcraft-web/Trunk.toml`),
so the site works at a domain root (`https://example.com/`), under a prefix
(`https://example.com/tools/soundcraft/`) and from a CDN bucket. The asset names carry a content
hash, so they can be cached forever. Only `index.html` needs revalidation.

## Required server settings

- **MIME type:** serve `.wasm` as `application/wasm`. Browsers refuse to stream-compile it under
  any other type, and the app then loads slowly or not at all. Serve `.js` as `text/javascript`.
  Most hosts already do both. For nginx, check that `mime.types` has `application/wasm wasm;`.
- **Compression:** turn on gzip or Brotli for `.wasm`, `.js` and `.html`. That takes the
  download to roughly a third. You can also precompress (`brotli -k *.wasm`) and let
  the server send `Content-Encoding: br`.
- **Caching:** `Cache-Control: public, max-age=31536000, immutable` on the hashed `.wasm` and
  `.js` files, and `no-cache` on `index.html`.
- **HTTPS:** WebGPU (and the clipboard) only work in a secure context, which means `https://`
  or `http://localhost`. Over plain HTTP elsewhere, the app falls back to WebGL2.
- **No special isolation headers:** SoundCraft plays audio through WebAudio without `SharedArrayBuffer`, so it doesn't
  need `Cross-Origin-Opener-Policy` or `Cross-Origin-Embedder-Policy`. If your site already sends
  COEP `require-corp`, also send `Cross-Origin-Resource-Policy: same-origin` (or `cross-origin`
  when the files live on a CDN) on the app's files.

nginx example:

```nginx
location /soundcraft/ {
    types { application/wasm wasm; text/javascript js; text/html html; }
    gzip on;
    gzip_types application/wasm text/javascript text/html;
    location ~* \.(wasm|js)$ { add_header Cache-Control "public, max-age=31536000, immutable"; }
    location ~* index\.html$ { add_header Cache-Control "no-cache"; }
}
```

Local test: `python3 -m http.server 8765` inside the folder, then open http://localhost:8765/.

## Embedding in a page (iframe)

```html
<iframe
  src="https://example.com/soundcraft/"
  title="SoundCraft"
  style="width: 100%; height: 720px; border: 0;"
  allow="fullscreen; autoplay; clipboard-read; clipboard-write"
  allowfullscreen>
</iframe>
```

- The app fills the iframe and follows its size, so size the iframe and not the app.
- Keyboard shortcuts go to the iframe after the user clicks into it, as with any embedded app.
- **Sandboxed iframes** need at least
  `sandbox="allow-scripts allow-same-origin"`.
- Don't send `X-Frame-Options: DENY` or a `frame-ancestors` CSP that excludes the embedding page.

## Renderer selection and fallback flags

SoundCraft renders with wgpu. It uses **WebGPU** when the browser has it and falls back to
**WebGL2** on its own. URL query flags override this, and they work on the iframe `src` too:

| Flag | Effect |
|---|---|
| *(none)* | WebGPU if available, otherwise WebGL2 |
| `?webgl` | Force the WebGL2 backend (useful when a WebGPU driver misbehaves) |
| `?empty` | Start with an empty session instead of the synthesised demo session |

For example: `<iframe src="https://example.com/soundcraft/?webgl" ...>`.

A browser with neither WebGPU nor WebGL2 gets a message in place of the app.

## Audio

Playback goes through WebAudio. Browsers only start an `AudioContext` after a user gesture, so
the first click or key press in the page may be needed before sound is heard. In an iframe,
`allow="autoplay"` lets the embedding page grant that up front. The web build has no recording
and no native file dialogs; it opens the synthesised demo session. There is no TCP control channel, but the page can drive the same one through `window.soundcraft` (below).

## Driving it from the page (`window.soundcraft`)

Every SoundCraft web build puts `window.soundcraft` on the page, from [`craft-web-api`](https://github.com/storytold/craft-libs/tree/main/crates/web-api) (storytold/craft-libs#413), the same shape in every Craft app. `soundcraft.request(method, params)` calls any method of the control channel the desktop app serves (`docs/control-protocol.md`), and `execute(command, params)`, `commands()`, `docInspect(params)`, `uiInspect(params)` and `screenshot(params)` are shortcuts for the common ones. Each returns a Promise that resolves with the method's result and rejects with an `Error`; a call the app does not answer within 30 seconds rejects. At most 64 calls wait for the app at once: a call made while that many wait rejects at once, as busy. SoundCraft's control requests cannot be withdrawn, so a call that times out in the page still runs when the app reaches it, as it does for a desktop client that timed out.

```js
const session = await soundcraft.docInspect();                       // tracks, clips, routing
await soundcraft.execute("track.new", { count: 2, format: "Stereo" });
const { data } = await soundcraft.screenshot();                      // the window as PNG (base64)
```

The API is installed in every session. Playback still needs a user gesture before the browser starts audio.

The repository holds a real-browser check of this API, `apps/soundcraft-web/tests/page-api.mjs`. It serves a built copy of the site (`trunk build --release` in `apps/soundcraft-web`, or the unpacked zip), loads it in headless Chromium with Playwright, and drives all six functions, a command that changes the session, the rejection of an unknown method and of an unknown command, the screenshot, several calls at once, and a burst of calls beyond the limit (the ones over it are refused as busy, and a call afterwards is accepted). Run it with `node apps/soundcraft-web/tests/page-api.mjs <site folder>` from a folder where `playwright` is installed. It is not part of `cargo xtask ci`.
