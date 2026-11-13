// Real-browser check of window.soundcraft (the page command API from craft-web-api): serve a built
// copy of the web app, load it in headless Chromium and drive every function the crate installs.
//
// Usage, from a folder where Playwright is installed (`npm i playwright`, then `npx playwright install chromium`):
//   node <repo>/apps/soundcraft-web/tests/page-api.mjs <site folder>
// The site folder is a built copy of the web app: `trunk build --release` in apps/soundcraft-web
// (dist/web), or the unpacked `packaging/web/package.sh` zip. Exits non-zero on the first failed check.
//
// Not repeated here: the 30 s request timeout and the 64-call queue limit belong to craft-web-api and are
// covered by that crate's own tests.
import { createRequire } from 'node:module'
import http from 'node:http'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import assert from 'node:assert/strict'

const dir = process.argv[2]
if (!dir) { console.error('usage: node page-api.mjs <site folder>'); process.exit(2) }
const require = createRequire(path.join(process.cwd(), 'noop.js'))
const { chromium } = (() => { try { return require('playwright') } catch { return require('@playwright/test') } })()

const types = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.png': 'image/png' }
const server = http.createServer(async (req, res) => {
  const route = decodeURIComponent(req.url.split('?')[0])
  const file = path.join(dir, route === '/' ? 'index.html' : route)
  try {
    const body = await readFile(file)
    res.writeHead(200, { 'content-type': types[path.extname(file)] || 'application/octet-stream' })
    res.end(body)
  } catch { res.writeHead(404); res.end() }
})
await new Promise((ok) => server.listen(0, '127.0.0.1', ok))
const origin = `http://127.0.0.1:${server.address().port}`
const browser = await chromium.launch({ headless: true, args: ['--disable-gpu'] })
try {
  const page = await (await browser.newContext({ viewport: { width: 1280, height: 900 } })).newPage()
  const errors = []
  page.on('pageerror', (e) => errors.push(e.message))
  // ?webgl forces the WebGL2 backend: headless Chromium has no WebGPU adapter.
  await page.goto(`${origin}/?webgl`)
  await page.locator('canvas').first().waitFor({ timeout: 180_000 })
  await page.waitForFunction(() => window.soundcraft && typeof window.soundcraft.request === 'function', null, { timeout: 180_000 })

  const names = await page.evaluate(() => Object.keys(window.soundcraft).sort())
  for (const f of ['request', 'execute', 'commands', 'docInspect', 'uiInspect', 'screenshot']) {
    assert(names.includes(f), `window.soundcraft.${f} is installed (found: ${names.join(' ')})`)
  }

  // A call that rejects is reported, not thrown, so each check can look at the error.
  const call = (fn, ...args) => page.evaluate(async ([fn, args]) => {
    try { return { ok: true, value: await window.soundcraft[fn](...args) } }
    catch (e) { return { ok: false, error: String((e && e.message) || e), isError: e instanceof Error } }
  }, [fn, args])

  const commands = await call('commands')
  assert(commands.ok && Array.isArray(commands.value) && commands.value.length > 0, `commands() lists the commands: ${JSON.stringify(commands).slice(0, 200)}`)
  assert(commands.value.some((c) => c.id === 'track.new'), 'commands() includes track.new')

  const doc = await call('docInspect')
  assert(doc.ok && typeof doc.value === 'object', `docInspect() resolves with an object: ${JSON.stringify(doc).slice(0, 200)}`)
  const ui = await call('uiInspect')
  assert(ui.ok && typeof ui.value === 'object', `uiInspect() resolves with an object: ${JSON.stringify(ui).slice(0, 200)}`)

  // execute changes the session, and docInspect shows it.
  const before = JSON.stringify((await call('docInspect')).value)
  const ran = await call('execute', 'track.new', {})
  assert(ran.ok, `execute('track.new') resolves: ${JSON.stringify(ran).slice(0, 300)}`)
  const after = JSON.stringify((await call('docInspect')).value)
  assert.notEqual(before, after, 'docInspect() changes after execute(track.new)')

  // Failures reject with an Error that names what went wrong.
  const unknownCommand = await call('execute', 'no.such.command', {})
  assert(!unknownCommand.ok && unknownCommand.isError && unknownCommand.error.includes('no.such.command'), `execute of an unknown command rejects with an Error naming it: ${JSON.stringify(unknownCommand)}`)
  const unknownMethod = await call('request', 'no.such.method', {})
  assert(!unknownMethod.ok && unknownMethod.isError && unknownMethod.error.includes('no.such.method'), `an unknown method rejects with an Error naming it: ${JSON.stringify(unknownMethod)}`)

  const shot = await call('screenshot')
  assert(shot.ok, `screenshot() resolves: ${JSON.stringify(shot).slice(0, 200)}`)
  const data = shot.value && shot.value.data
  assert(typeof data === 'string' && data.length > 1000, 'screenshot() carries base64 data')
  assert.equal(Buffer.from(data, 'base64').subarray(1, 4).toString(), 'PNG', 'the screenshot is a PNG')

  // Several calls at once all settle.
  const settled = await page.evaluate(() => Promise.all([1, 2, 3, 4, 5].map(() => window.soundcraft.uiInspect())).then((r) => r.length))
  assert.equal(settled, 5, 'five concurrent calls all settle')

  assert.deepEqual(errors, [], 'no uncaught page errors')
  console.log('PASS window.soundcraft: all six functions, the failure paths and concurrent calls work against the real app')
} finally {
  await browser.close()
  server.close()
}
