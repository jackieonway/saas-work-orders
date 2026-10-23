import assert from 'node:assert/strict'
import test from 'node:test'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, realpathSync, rmSync } from 'node:fs'
import { createServer } from 'node:http'
import path from 'node:path'
import os from 'node:os'
import vm from 'node:vm'
import { fileURLToPath } from 'node:url'

test('qiankun release is a classic UMD lifecycle with complete relative CSS and no dynamic chunks', { timeout: 90000 }, async t => {
  const temporary = realpathSync(os.tmpdir()), root = realpathSync(mkdtempSync(path.join(temporary, 'platform-qiankun-work-orders-')))
  t.after(() => {
    assert.equal(path.dirname(realpathSync(root)), temporary)
    assert.ok(path.basename(root).startsWith('platform-qiankun-work-orders-'))
    rmSync(root, { recursive: true, force: true })
  })
  const output = path.join(root, 'release'), builder = fileURLToPath(new URL('../scripts/build.mjs', import.meta.url))
  const built = spawnSync(process.execPath, [builder, '--out-dir', output, '--allowed-host-origin', 'https://platform.example.com'], { encoding: 'utf8', windowsHide: true, timeout: 90000 })
  assert.equal(built.status, 0, built.stdout + built.stderr)
  const info = JSON.parse(readFileSync(path.join(output, 'build-info.json'), 'utf8'))
  assert.equal(info.runtime, 'qiankun')
  assert.deepEqual(info.dynamicChunks, [])
  const html = readFileSync(path.join(output, 'index.html'), 'utf8')
  assert.match(html, /<div data-work-orders-mount>/)
  assert.match(html, /<script entry src="\.\/assets\/micro-app\.umd\.js"><\/script>/)
  assert.match(html, /<link rel="stylesheet" href="\.\/[^"\s]+\.css">/)
  assert.doesNotMatch(html, /type="module"|modulepreload|iframe-bridge|assets\/config/)
  const javascript = readFileSync(path.join(output, info.entry), 'utf8')
  assert.doesNotMatch(javascript, /\bimport\s*\(/)
  const sandbox = vm.createContext({ console, fetch, AbortController, DOMException, Headers, URL, TextEncoder, TextDecoder, ReadableStream, Response, Request, crypto, setTimeout, clearTimeout })
  new vm.Script(javascript, { filename: info.entry }).runInContext(sandbox)
  assert.deepEqual(Object.keys(sandbox.WorkOrdersMicroApp).sort(), ['bootstrap', 'mount', 'unmount', 'update'])
  for (const lifecycle of Object.values(sandbox.WorkOrdersMicroApp)) assert.equal(typeof lifecycle, 'function')
  // import-html-entry may select a framework's later global mutation instead
  // of the UMD object. qiankun then falls back to the sandbox's latest set prop;
  // this must work independently of its per-mount application name.
  let latestSetProp
  const proxy = new Proxy({}, { set(target, property, value) { latestSetProp = property; target[property] = value; return true } })
  const strictSandbox = vm.createContext({ console, fetch, AbortController, DOMException, Headers, URL, TextEncoder, TextDecoder, ReadableStream, Response, Request, crypto, setTimeout, clearTimeout, window: proxy, self: proxy, globalThis: proxy })
  new vm.Script(javascript, { filename: info.entry }).runInContext(strictSandbox)
  assert.equal(latestSetProp, 'WorkOrdersMicroApp')
  assert.notEqual(proxy['work-orders-runtime-random-name'], proxy[latestSetProp])
  assert.deepEqual(Object.keys(proxy[latestSetProp]).sort(), ['bootstrap', 'mount', 'unmount', 'update'])

  const prefix = '/micro-proxy/work-orders/versions/test/'
  const server = createServer((request, response) => {
    const name = new URL(request.url, 'http://localhost').pathname.slice(prefix.length)
    if (!request.url.startsWith(prefix) || name.includes('..')) { response.writeHead(404); response.end(); return }
    try {
      response.setHeader('Content-Type', name.endsWith('.css') ? 'text/css' : name.endsWith('.html') ? 'text/html' : 'text/javascript')
      response.end(readFileSync(path.join(output, name)))
    } catch { response.writeHead(404); response.end() }
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => new Promise(resolve => { server.closeAllConnections(); server.close(resolve) }))
  const base = new URL(prefix, `http://127.0.0.1:${server.address().port}`)
  for (const [, resource] of html.matchAll(/(?:href|src)="(\.\/[^"#]+)"/g)) {
    const url = new URL(resource, base)
    assert.ok(url.pathname.startsWith(prefix))
    const response = await fetch(url)
    assert.equal(response.status, 200, resource)
    assert.equal(await response.text(), readFileSync(path.join(output, resource), 'utf8'))
  }
})
