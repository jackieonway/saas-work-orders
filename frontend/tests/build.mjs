import assert from 'node:assert/strict'
import test from 'node:test'
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const builder = fileURLToPath(new URL('../scripts/build.mjs', import.meta.url))
const appRoot = fileURLToPath(new URL('../', import.meta.url))
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex')
function build(t, origin, api = '/work-orders-api', extra = {}) {
  const temp = realpathSync(os.tmpdir()), root = realpathSync(mkdtempSync(path.join(temp, 'platform-work-orders-build-test-')))
  t.after(() => {
    const relative = path.relative(temp, root)
    assert.ok(relative && !path.isAbsolute(relative) && !relative.startsWith('..' + path.sep))
    assert.ok(path.basename(root).startsWith('platform-work-orders-build-test-'))
    assert.equal(realpathSync(root), root)
    rmSync(root, { recursive: true, force: true })
  })
  const output = path.join(root, 'release')
  extra.prepare?.(output)
  const result = spawnSync(process.execPath, [builder, '--allowed-host-origin', origin, '--api-base', api, '--out-dir', output],
    { encoding: 'utf8', windowsHide: true, timeout: 60000, env: { ...process.env, ...extra.env } })
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

test('production release contains a relative ESM graph, a dynamic application chunk, and exact source/SDK metadata', t => {
  const secret = 'phase21-private-sentinel-never-public'
  const result = build(t, 'https://platform.example.com', '/work-orders-api', { env: {
    VITE_CLIENT_SECRET: secret, DATABASE_URL: secret, WORK_ORDERS_CLIENT_SECRET: secret,
  } })
  assert.equal(result.status, 0, result.stderr)
  const json = name => JSON.parse(readFileSync(path.join(result.output, name), 'utf8'))
  const info = json('build-info.json'), manifest = json('asset-manifest.json')
  assert.equal(info.application, 'work-orders')
  assert.equal(info.sdkVersion, '1.0.0')
  assert.equal(info.vueVersion, '3.5.43')
  assert.equal(info.viteVersion, '8.3.4')
  assert.equal(info.inputCount, info.inputs.length)
  assert.equal(info.sourceSha256, sha256(JSON.stringify(info.inputs)))
  for (const input of info.inputs) {
    const bytes = readFileSync(path.join(appRoot, input.path))
    assert.equal(input.bytes, bytes.length)
    assert.equal(input.sha256, sha256(bytes), input.path)
  }
  assert.ok(info.inputs.some(input => input.path === 'src/main.mjs'))
  assert.ok(!info.inputs.some(input => input.path.includes('vue.global')))
  assert.equal(info.packageLockSha256, sha256(readFileSync(path.join(appRoot, 'package-lock.json'))))
  assert.equal(manifest['index.html'].isEntry, true)
  assert.equal(manifest['index.html'].file, info.entry)
  assert.deepEqual(manifest['index.html'].dynamicImports.map(key => manifest[key].file).sort(), info.dynamicChunks)
  assert.ok(info.dynamicChunks.length > 0, 'an actual deferred application is emitted')
  const html = readFileSync(path.join(result.output, 'index.html'), 'utf8')
  assert.match(html, /<script type="module"[^>]*src="\.\/assets\//)
  assert.match(html, /src="\.\/assets\/iframe-bridge\.js"/)
  assert.match(html, /src="\.\/assets\/config\.js"/)
  assert.doesNotMatch(html, /vue\.global|src="\/|href="\//)
  const entry = readFileSync(path.join(result.output, info.entry), 'utf8')
  assert.match(entry, /import\((["'`])\.\/app-[^"'`]+\.js\1\)/)
  for (const record of Object.values(manifest)) {
    for (const resource of [record.file, ...(record.css || [])]) {
      assert.ok(resource.startsWith('assets/') && !resource.includes('..'))
      const source = readFileSync(path.join(result.output, resource), 'utf8')
      assert.ok(!source.includes(secret), 'private environment values are absent')
      assert.doesNotMatch(source, /window\.Vue/)
    }
  }
  const config = readFileSync(path.join(result.output, 'assets/config.js'), 'utf8')
  assert.ok(!config.includes(secret))
  assert.match(config, /"apiBase":"\/work-orders-api"/)
  assert.equal(readFileSync(path.join(result.output, 'assets/iframe-bridge.js'), 'utf8'), readFileSync(path.join(appRoot, 'vendor/iframe-bridge.js'), 'utf8'))
  assert.equal(existsSync(path.join(result.output, 'assets/vue.global.prod.js')), false)
})

test('a pre-existing unowned output directory is preserved', t => {
  const result = build(t, 'https://platform.example.com', '/work-orders-api', { prepare(output) {
    mkdirSync(output); writeFileSync(path.join(output, 'keep.txt'), 'foreign output')
  } })
  assert.notEqual(result.status, 0)
  assert.match(result.stderr, /不属于本应用构建/)
  assert.equal(readFileSync(path.join(result.output, 'keep.txt'), 'utf8'), 'foreign output')
})

test('output outside the owned dist and temporary directory is rejected before writing', () => {
  const output = path.join(appRoot, 'must-not-be-created')
  assert.equal(existsSync(output), false)
  const result = spawnSync(process.execPath, [builder, '--allowed-host-origin', 'https://platform.example.com', '--out-dir', output],
    { encoding: 'utf8', windowsHide: true, timeout: 60000 })
  assert.notEqual(result.status, 0)
  assert.match(result.stderr, /自定义输出须位于系统临时目录内/)
  assert.equal(existsSync(output), false)
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
