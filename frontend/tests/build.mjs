import assert from 'node:assert/strict'
import test from 'node:test'
import { spawnSync } from 'node:child_process'
import { existsSync, mkdtempSync, realpathSync, rmSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const builder = fileURLToPath(new URL('../scripts/build.mjs', import.meta.url))
function build(t, origin, api = '/work-orders-api') {
  const temp = realpathSync(os.tmpdir()), root = realpathSync(mkdtempSync(path.join(temp, 'platform-work-orders-build-test-')))
  t.after(() => {
    const relative = path.relative(temp, root)
    assert.ok(relative && !path.isAbsolute(relative) && !relative.startsWith('..' + path.sep))
    assert.ok(path.basename(root).startsWith('platform-work-orders-build-test-'))
    assert.equal(realpathSync(root), root)
    rmSync(root, { recursive: true, force: true })
  })
  const output = path.join(root, 'release')
  const result = spawnSync(process.execPath, [builder, '--allowed-host-origin', origin, '--api-base', api, '--out-dir', output],
    { encoding: 'utf8', windowsHide: true, timeout: 10000 })
  return { ...result, output }
}

test('remote HTTP hosts and APIs are rejected before producing a release', (t) => {
  for (const [origin, api] of [
    ['http://platform.example.com', '/work-orders-api'],
    ['http://localhost.example.com', '/work-orders-api'],
    ['https://platform.example.com', 'http://api.example.com/work-orders-api'],
  ]) {
    const result = build(t, origin, api)
    assert.notEqual(result.status, 0)
    assert.match(result.stderr, /必须使用 HTTPS/)
    assert.equal(existsSync(result.output), false)
  }
})

test('HTTPS and literal loopback HTTP configurations remain buildable', (t) => {
  for (const [origin, api] of [
    ['https://platform.example.com', 'https://api.example.com/work-orders-api'],
    ['http://localhost:3000', '/work-orders-api'],
    ['http://127.0.0.1:3000', 'http://127.0.0.1:3001/work-orders-api'],
    ['http://[::1]:3000', 'http://[::1]:3001/work-orders-api'],
  ]) {
    const result = build(t, origin, api)
    assert.equal(result.status, 0, result.stderr)
    assert.equal(existsSync(path.join(result.output, 'index.html')), true)
  }
})
