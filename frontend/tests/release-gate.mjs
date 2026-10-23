import assert from 'node:assert/strict'
import test from 'node:test'
import { spawnSync } from 'node:child_process'
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { checkRelease, releaseFiles, releaseHashFile } from '../scripts/release-check.mjs'
import { createReleaseCandidate } from '../scripts/release-candidate.mjs'

const builder = fileURLToPath(new URL('../scripts/build.mjs', import.meta.url))
const appRoot = fileURLToPath(new URL('../', import.meta.url))
const readJson = file => JSON.parse(readFileSync(file, 'utf8'))
const writeJson = (file, value) => writeFileSync(file, JSON.stringify(value, null, 2) + '\n')
function temporary(t) {
  const parent = realpathSync(os.tmpdir()), root = realpathSync(mkdtempSync(path.join(parent, 'work-orders-release-gate-test-')))
  t.after(() => {
    assert.equal(path.dirname(realpathSync(root)), parent)
    assert.ok(path.basename(root).startsWith('work-orders-release-gate-test-'))
    rmSync(root, { recursive: true, force: true })
  })
  return root
}
function reseal(directory) {
  const inventory = readJson(path.join(directory, releaseHashFile))
  inventory.files = releaseFiles(directory).filter(file => file.path !== releaseHashFile)
  writeJson(path.join(directory, releaseHashFile), inventory)
}

test('release gate checks actual UMD assets and fails closed for stale, incomplete or legacy releases', { timeout: 120000 }, async t => {
  const root = temporary(t), output = path.join(root, 'valid')
  const built = spawnSync(process.execPath, [builder, '--out-dir', output, '--allowed-host-origin', 'https://platform.example.com'],
    { encoding: 'utf8', windowsHide: true, timeout: 90000 })
  assert.equal(built.status, 0, built.stdout + built.stderr)
  assert.match(built.stdout, /Release gate passed/)
  const report = await checkRelease(output)
  assert.equal(report.runtime, 'qiankun')
  assert.equal(report.sdkVersion, '2.0.0')
  assert.equal(report.version, readJson(path.join(appRoot, 'package.json')).version)
  const cli = spawnSync(process.execPath, [fileURLToPath(new URL('../scripts/release-check.mjs', import.meta.url)), output],
    { encoding: 'utf8', windowsHide: true, timeout: 30000 })
  assert.equal(cli.status, 0, cli.stdout + cli.stderr)
  assert.equal(JSON.parse(cli.stdout).directory, realpathSync(output))

  let count = 0
  const reject = async (name, mutate, { seal = true, pattern } = {}) => {
    await t.test(name, async () => {
      const broken = path.join(root, `broken-${++count}`)
      cpSync(output, broken, { recursive: true, errorOnExist: true, force: false })
      mutate(broken)
      if (seal) reseal(broken)
      await assert.rejects(checkRelease(broken), pattern)
    })
  }
  for (const file of ['index.html', 'assets/micro-app.umd.js', 'micro-app.css']) {
    await reject(`missing ${file}`, directory => rmSync(path.join(directory, file)), { seal: false })
  }
  await reject('corrupted UMD hash', directory => writeFileSync(path.join(directory, 'assets/micro-app.umd.js'), 'corrupt'), { seal: false, pattern: /file hashes mismatch/ })
  await reject('incorrect inventory digest', directory => {
    const file = path.join(directory, releaseHashFile), inventory = readJson(file)
    inventory.files[0].sha256 = '0'.repeat(64); writeJson(file, inventory)
  }, { seal: false, pattern: /file hashes mismatch/ })
  await reject('SDK 1 metadata', directory => {
    const file = path.join(directory, 'build-info.json'), info = readJson(file)
    info.sdkVersion = '1.0.0'; writeJson(file, info)
  }, { pattern: /SDK 2\.0\.0/ })
  await reject('SDK 1 runtime despite SDK 2 metadata and matching hashes', directory => {
    const file = path.join(directory, 'assets/micro-app.umd.js')
    const javascript = readFileSync(file, 'utf8')
    assert.ok(javascript.includes('Object.freeze([`2.0.0`])'))
    writeFileSync(file, javascript.replace('Object.freeze([`2.0.0`])', 'Object.freeze([`1.0.0`])'))
  }, { pattern: /executed UMD does not support exactly SDK 2\.0\.0/ })
  await reject('stale source fingerprint', directory => {
    const file = path.join(directory, 'build-info.json'), info = readJson(file)
    info.inputs.find(input => input.path === 'src/app.mjs').sha256 = '0'.repeat(64); writeJson(file, info)
  }, { pattern: /current source input hashes mismatch/ })
  await reject('old ESM index', directory => {
    const file = path.join(directory, 'index.html')
    writeFileSync(file, readFileSync(file, 'utf8').replace('<script entry', '<script type="module" entry'))
  }, { pattern: /HTML is not the relative qiankun entry/ })
  await reject('iframe entry', directory => {
    const file = path.join(directory, 'index.html')
    writeFileSync(file, readFileSync(file, 'utf8') + '<iframe src="legacy.html"></iframe>')
  }, { pattern: /HTML is not the relative qiankun entry/ })
  await reject('unreferenced stale ESM chunk', directory => writeFileSync(path.join(directory, 'assets/legacy.mjs'), 'export const mount=()=>{}'), { pattern: /legacy or split JavaScript/ })
  await reject('CSS reference missing from HTML', directory => {
    const file = path.join(directory, 'index.html')
    writeFileSync(file, readFileSync(file, 'utf8').replace(/<link[^>]+>/g, ''))
  }, { pattern: /complete CSS/ })
  await reject('invalid UMD lifecycle despite matching file hashes', directory => {
    const file = path.join(directory, 'assets/micro-app.umd.js')
    writeFileSync(file, readFileSync(file, 'utf8') + '\nWorkOrdersMicroApp.mount = async () => {};\n')
  }, { pattern: /accepted a missing platform context/ })
  await reject('metadata version changed independently from public config', directory => {
    const file = path.join(directory, 'build-info.json'), info = readJson(file)
    info.version = 'old-release'; writeJson(file, info)
  }, { pattern: /inventory metadata mismatch/ })
  await assert.rejects(checkRelease(output, { expectedVersion: 'unexpected' }), /version mismatch/)
})

test('local candidate contains checked assets and public nginx configuration and refuses all existing versions', { timeout: 120000 }, async t => {
  const root = temporary(t), candidateRoot = path.join(root, 'candidates')
  const input = { version: '2.0.0-candidate-test', hostOrigin: 'https://platform.example.com', apiBase: '/work-orders-api' }
  const created = await createReleaseCandidate(input, { candidateRoot })
  assert.equal(created.directory, path.join(candidateRoot, 'work-orders', 'versions', input.version))
  assert.equal(created.metadata.state, 'local-candidate')
  assert.equal(created.metadata.deployed, false)
  assert.equal(created.metadata.runtime, 'qiankun')
  assert.equal(created.metadata.sdkVersion, '2.0.0')
  assert.deepEqual(created.metadata.publicConfig, { appCode: 'work-orders', ...input })
  const metadata = readJson(path.join(created.directory, 'candidate.json'))
  assert.deepEqual(metadata.files, releaseFiles(created.directory).filter(file => file.path !== 'candidate.json'))
  assert.match(readFileSync(path.join(created.directory, 'nginx.conf'), 'utf8'), /Access-Control-Allow-Origin "\*"/)
  assert.match(readFileSync(path.join(created.directory, 'nginx.conf'), 'utf8'), /try_files \$uri \$uri\/index\.html =404/)
  assert.equal((await checkRelease(created.staticDirectory)).version, input.version)
  const before = readFileSync(path.join(created.directory, 'candidate.json'), 'utf8')
  await assert.rejects(createReleaseCandidate(input, { candidateRoot }), /refusing to overwrite/)
  assert.equal(readFileSync(path.join(created.directory, 'candidate.json'), 'utf8'), before)
  const incomplete = { ...input, version: '2.0.0-existing-empty' }
  mkdirSync(path.join(candidateRoot, 'work-orders', 'versions', incomplete.version))
  await assert.rejects(createReleaseCandidate(incomplete, { candidateRoot }), /refusing to overwrite/)
  await assert.rejects(createReleaseCandidate({ ...input, version: '../escape' }, { candidateRoot }), /Invalid immutable/)
  await assert.rejects(createReleaseCandidate({ ...input, version: '2.0.0-invalid', hostOrigin: 'http://platform.example.com' }, { candidateRoot }), /exact HTTPS/)
  assert.equal(existsSync(path.join(candidateRoot, 'work-orders', 'versions', '2.0.0-invalid')), false)
})

test('explicit local candidate permits only loopback HTTP and does not emit a production nginx configuration', { timeout: 120000 }, async t => {
  const root = temporary(t), candidateRoot = path.join(root, 'local-candidates')
  const input = { version: '2.0.0-local-test', hostOrigin: 'http://127.0.0.1:3000', apiBase: '/work-orders-api' }
  const created = await createReleaseCandidate(input, { candidateRoot, local: true })
  assert.equal(created.metadata.localCandidate, true)
  assert.equal(created.metadata.deployed, false)
  assert.equal(created.metadata.purpose, 'loopback-development-validation')
  assert.equal(created.metadata.nginxConfiguration, null)
  assert.equal(existsSync(path.join(created.directory, 'nginx.conf')), false)
  assert.equal((await checkRelease(created.staticDirectory)).version, input.version)
  await assert.rejects(createReleaseCandidate({ ...input, version: '2.0.0-http-production' }, { candidateRoot }), /exact HTTPS/)
  for (const extra of [
    { hostOrigin: 'http://platform.example.com' },
    { hostOrigin: 'https://platform.example.com' },
    { apiBase: 'http://api.example.com/work-orders-api' },
  ]) {
    await assert.rejects(createReleaseCandidate({ ...input, ...extra, version: '2.0.0-local-invalid' }, { candidateRoot, local: true }), /only exact loopback HTTP/)
  }
})
