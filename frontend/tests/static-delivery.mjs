import assert from 'node:assert/strict'
import test from 'node:test'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, realpathSync, rmSync } from 'node:fs'
import path from 'node:path'
import os from 'node:os'
import { fileURLToPath } from 'node:url'
import { preview } from 'vite'
import { createWorkOrdersConfig, readPublicConfig } from '../vite.config.mjs'

test('version-prefixed static delivery serves the ESM graph and keeps missing resources as real 404s', { timeout: 60000 }, async t => {
  const temporary = realpathSync(os.tmpdir())
  const root = realpathSync(mkdtempSync(path.join(temporary, 'work-orders-vite-static-')))
  t.after(() => {
    assert.equal(path.dirname(root), temporary)
    assert.ok(path.basename(root).startsWith('work-orders-vite-static-'))
    assert.equal(realpathSync(root), root)
    rmSync(root, { recursive: true, force: true })
  })
  const outDir = path.join(root, 'release')
  const builder = fileURLToPath(new URL('../scripts/build.mjs', import.meta.url))
  const result = spawnSync(process.execPath, [builder, '--version', '2.1.0-static-test', '--out-dir', outDir,
    '--allowed-host-origin', 'https://platform.example.com'], { encoding: 'utf8', windowsHide: true, timeout: 60000 })
  assert.equal(result.status, 0, result.stderr)
  const prefix = '/sub-apps/work-orders/versions/2.1.0-static-test/'
  const config = createWorkOrdersConfig(readPublicConfig({ version: '2.1.0-static-test', origins: ['https://platform.example.com'] }))
  const server = await preview({ ...config, configFile: false, base: prefix,
    build: { ...config.build, outDir }, preview: { host: '127.0.0.1', port: 0, strictPort: true } })
  t.after(async () => {
    server.httpServer.closeAllConnections()
    await new Promise((resolve, reject) => server.httpServer.close(error => error ? reject(error) : resolve()))
  })
  const origin = `http://127.0.0.1:${server.httpServer.address().port}`
  const base = new URL(prefix, origin)
  const htmlResponse = await fetch(new URL('index.html', base))
  assert.equal(htmlResponse.status, 200)
  const html = await htmlResponse.text()
  const urls = [...html.matchAll(/(?:src|href)="(\.\/[^"#]+)"/g)].map(match => match[1])
  assert.ok(urls.length >= 4)
  for (const resource of urls) {
    const url = new URL(resource, base)
    assert.ok(url.pathname.startsWith(prefix))
    const response = await fetch(url)
    assert.equal(response.status, 200, resource)
    assert.equal(await response.text(), readFileSync(path.join(outDir, resource), 'utf8'))
  }
  const metadata = JSON.parse(readFileSync(path.join(outDir, 'build-info.json'), 'utf8'))
  for (const chunk of metadata.dynamicChunks) {
    const response = await fetch(new URL(chunk, base))
    assert.equal(response.status, 200)
    assert.match(response.headers.get('content-type'), /javascript/)
    assert.equal(await response.text(), readFileSync(path.join(outDir, chunk), 'utf8'))
  }
  for (const missing of ['assets/missing.js', 'assets/missing.css', 'missing.html', 'tickets/arbitrary']) {
    const response = await fetch(new URL(missing, base), { headers: { Accept: 'text/html' } })
    assert.equal(response.status, 404, missing)
    assert.ok(!(await response.text()).includes('正在连接当前组织的工单空间'))
  }
})
