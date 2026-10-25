import { createHash } from 'node:crypto'
import { lstatSync, readFileSync, readdirSync, realpathSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import vm from 'node:vm'

const appRoot = fileURLToPath(new URL('../', import.meta.url))
export const releaseHashFile = 'release-files.json'
const hash = bytes => createHash('sha256').update(bytes).digest('hex')
const fail = message => { throw new Error(`Release gate: ${message}`) }
const json = file => JSON.parse(readFileSync(file, 'utf8'))
const literalPath = value => typeof value === 'string' && /^[A-Za-z0-9_.@/-]+$/.test(value) &&
  !value.startsWith('/') && value.split('/').every(segment => segment && segment !== '.' && segment !== '..')

export function releaseFiles(directory) {
  if (lstatSync(directory).isSymbolicLink() || !lstatSync(directory).isDirectory()) fail('release must be a real directory')
  const files = []
  function visit(relative = '') {
    for (const entry of readdirSync(path.join(directory, relative), { withFileTypes: true })) {
      const name = relative ? `${relative}/${entry.name}` : entry.name
      if (!literalPath(name) || entry.isSymbolicLink()) fail(`unsafe release path: ${name}`)
      if (entry.isDirectory()) visit(name)
      else if (entry.isFile()) {
        const bytes = readFileSync(path.join(directory, name))
        files.push({ path: name, bytes: bytes.length, sha256: hash(bytes) })
      } else fail(`unsupported release file: ${name}`)
    }
  }
  visit()
  return files.sort((left, right) => left.path.localeCompare(right.path, 'en'))
}

function sourceFiles(sourceRoot) {
  const files = ['index.html', 'package.json', 'package-lock.json', 'vite.config.mjs', 'vendor/vue.LICENSE.txt']
  const visit = relative => {
    for (const entry of readdirSync(path.join(sourceRoot, relative), { withFileTypes: true })) {
      const name = `${relative}/${entry.name}`
      if (entry.isSymbolicLink()) fail(`source symlink: ${name}`)
      if (entry.isDirectory()) visit(name)
      else if (entry.isFile()) files.push(name)
      else fail(`unsupported source file: ${name}`)
    }
  }
  visit('src'); visit('scripts'); visit('vendor/micro-sdk')
  return files.sort().map(name => {
    const bytes = readFileSync(path.join(sourceRoot, name))
    return { path: name, bytes: bytes.length, sha256: hash(bytes) }
  })
}

function compareRecords(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) fail(`${label} mismatch`)
}

async function checkLifecycle(javascript, info) {
  // Do not provide Node imports, file access, credentials, or a live network.
  // VM inspection is a format/lifecycle check, not an untrusted-code sandbox.
  const blockedFetch = () => { throw new Error('Release gate forbids network access') }
  const sandbox = vm.createContext({ console, fetch: blockedFetch, AbortController, DOMException, Headers, URL,
    TextEncoder, TextDecoder, ReadableStream, Response, Request, crypto, setTimeout, clearTimeout })
  new vm.Script(`globalThis.__gateSdkVersions = [];
    { const original = Object.freeze; Object.freeze = function(value) {
      if (Array.isArray(value) && value.some(item => typeof item === 'string' && /^\\d+\\.\\d+\\.\\d+$/.test(item))) {
        globalThis.__gateSdkVersions.push(Array.from(value));
      }
      return original(value);
    }; }`).runInContext(sandbox, { timeout: 5000 })
  const script = new vm.Script(javascript, { filename: info.entry })
  script.runInContext(sandbox, { timeout: 5000 })
  if (JSON.stringify(sandbox.__gateSdkVersions) !== '[["3.0.0"]]') fail('executed UMD does not support exactly SDK 3.0.0')
  const lifecycle = sandbox.WorkOrdersMicroApp
  if (!lifecycle || Object.keys(lifecycle).sort().join(',') !== 'bootstrap,mount,unmount,update' ||
    Object.values(lifecycle).some(value => typeof value !== 'function')) fail('UMD must export exactly bootstrap/mount/update/unmount')
  sandbox.__gateLifecycle = lifecycle
  async function call(expression, rejects = false) {
    let value
    let timer
    try {
      value = new vm.Script(expression).runInContext(sandbox, { timeout: 5000 })
      await Promise.race([value, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('Release gate lifecycle timed out')), 5000) })])
    }
    catch (error) { if (rejects && /props\.platform/.test(error.message)) return; throw error }
    finally { clearTimeout(timer) }
    if (rejects) fail('lifecycle accepted a missing platform context')
  }
  await call('__gateLifecycle.bootstrap()')
  await call('__gateLifecycle.mount({})', true)
  await call('__gateLifecycle.update({})', true)
  const controller = new AbortController(); controller.abort()
  sandbox.__gateAbortedSignal = controller.signal
  await call('__gateLifecycle.mount({platform:{signal:__gateAbortedSignal}})')
  await call('__gateLifecycle.update({platform:{signal:__gateAbortedSignal}})')
  await call('__gateLifecycle.unmount()')

  // qiankun falls back to the last global set by UMD execution. Vue also
  // mutates globals; the final assignment must restore this lifecycle object.
  let latest
  const window = new Proxy({}, { set(target, key, value) { latest = key; target[key] = value; return true } })
  const proxySandbox = vm.createContext({ console, fetch: blockedFetch, AbortController, DOMException, Headers, URL,
    TextEncoder, TextDecoder, ReadableStream, Response, Request, crypto, setTimeout, clearTimeout,
    window, self: window, globalThis: window })
  script.runInContext(proxySandbox, { timeout: 5000 })
  if (latest !== 'WorkOrdersMicroApp' || Object.keys(window[latest] || {}).sort().join(',') !== 'bootstrap,mount,unmount,update') {
    fail('qiankun latest global does not resolve to the UMD lifecycle')
  }
}

export async function checkRelease(directory = path.join(appRoot, 'dist'), { sourceRoot = appRoot, expectedVersion, expectedPublicConfig } = {}) {
  const root = realpathSync(directory), source = realpathSync(sourceRoot)
  // Resolve every directory from disk, and reject symlinks before using any
  // metadata paths. Unexpected files are a failure, including stale ESM chunks.
  const files = releaseFiles(directory)
  for (const name of ['index.html', 'build-info.json', 'asset-manifest.json', 'assets/micro-app.umd.js', 'assets/vue.LICENSE.txt', releaseHashFile]) {
    if (!files.some(file => file.path === name)) fail(`missing required release file: ${name}`)
  }
  const info = json(path.join(root, 'build-info.json'))
  const inventory = json(path.join(root, releaseHashFile))
  if (info.application !== 'work-orders' || info.runtime !== 'qiankun' || info.format !== 'umd' ||
    info.contractVersion !== '2.0' || info.sdkVersion !== '3.0.0' || info.releaseGateVersion !== 1) fail('requires qiankun UMD, MicroProps 2.0 and SDK 3.0.0 metadata')
  if (typeof info.version !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(info.version) || info.version.includes('..') ||
    (expectedVersion && info.version !== expectedVersion)) fail('application version mismatch')
  const pkg = json(path.join(source, 'package.json')), lock = json(path.join(source, 'package-lock.json'))
  if (info.packageVersion !== pkg.version || lock.version !== pkg.version || lock.packages?.['']?.version !== pkg.version) fail('package version mismatch')
  if (inventory.schemaVersion !== 1 || inventory.application !== 'work-orders' || inventory.version !== info.version) fail('file inventory metadata mismatch')
  compareRecords(inventory.files, files.filter(file => file.path !== releaseHashFile), 'release file hashes')
  const inputs = sourceFiles(source)
  compareRecords(info.inputs, inputs, 'current source input hashes')
  if (info.inputCount !== inputs.length || info.sourceSha256 !== hash(JSON.stringify(inputs)) ||
    info.packageLockSha256 !== inputs.find(input => input.path === 'package-lock.json').sha256) fail('source fingerprint mismatch')
  if (!inputs.some(input => input.path === 'src/qiankun-entry.mjs') || inputs.some(input => /(?:iframe-bridge|src\/(?:main|lifecycle)\.mjs|vendor\/micro-sdk\.mjs)$/.test(input.path))) fail('legacy source entry present')
  const sdk = json(path.join(source, 'vendor/micro-sdk/package.json')), sdkSource = json(path.join(source, 'vendor/micro-sdk/SOURCE.json'))
  if (sdk.version !== '3.0.0' || sdkSource.sdkVersion !== '3.0.0' || !Array.isArray(sdkSource.files)) fail('SDK source is not 2.0.0')
  for (const record of sdkSource.files) {
    if (!literalPath(record.path)) fail('unsafe SDK provenance path')
    const bytes = readFileSync(path.join(source, 'vendor/micro-sdk', record.path))
    if (record.bytes !== bytes.length || record.sha256 !== hash(bytes)) fail(`SDK provenance hash mismatch: ${record.path}`)
  }
  const config = info.publicConfig
  if (!config || config.version !== info.version || typeof config.apiBase !== 'string' || !Array.isArray(config.allowedHostOrigins) || !config.allowedHostOrigins.length) fail('public build configuration is missing')
  for (const origin of config.allowedHostOrigins) {
    const url = new URL(origin)
    if (url.origin !== origin || !['https:', 'http:'].includes(url.protocol) ||
      (url.protocol === 'http:' && !['localhost', '127.0.0.1', '[::1]'].includes(url.hostname))) fail('invalid host origin')
  }
  if (expectedPublicConfig) compareRecords(config, expectedPublicConfig, 'public build configuration')
  if (info.entry !== 'assets/micro-app.umd.js' || info.buildManifest !== 'asset-manifest.json' ||
    !Array.isArray(info.dynamicChunks) || info.dynamicChunks.length) fail('release must have one classic UMD entry and no dynamic chunks')
  const manifest = json(path.join(root, info.buildManifest)), records = Object.values(manifest)
  const entry = manifest['src/qiankun-entry.mjs']
  if (!entry?.isEntry || entry.file !== info.entry || records.filter(record => record.isEntry).length !== 1 ||
    records.some(record => record.isDynamicEntry || record.imports?.length || record.dynamicImports?.length)) fail('Vite manifest contains unexpected entry/chunks')
  const html = readFileSync(path.join(root, 'index.html'), 'utf8')
  if (/<iframe\b|iframe-bridge|\btype\s*=\s*["']module|modulepreload|(?:href|src)\s*=\s*["'](?:\/|https?:|data:|javascript:)/i.test(html) ||
    !/<div data-work-orders-mount><\/div>/.test(html)) fail('HTML is not the relative qiankun entry')
  const scripts = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script\s*>/gi)]
  if (scripts.length !== 1 || scripts[0][2].trim() || !/^\s*entry\s+src="\.\/assets\/micro-app\.umd\.js"\s*$/.test(scripts[0][1])) fail('HTML requires exactly one external classic entry script')
  const css = [...html.matchAll(/<link\s+rel="stylesheet"\s+href="\.\/([^"\s]+\.css)"\s*>/g)].map(match => match[1]).sort()
  const actualCss = files.filter(file => file.path.endsWith('.css')).map(file => file.path).sort()
  if (!css.length || new Set(css).size !== css.length) fail('HTML must include the complete CSS')
  compareRecords(css, actualCss, 'HTML CSS references')
  const expectedHtml = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>工单中心</title>${css.map(name => `<link rel="stylesheet" href="./${name}">`).join('')}</head><body><div data-work-orders-mount></div><script entry src="./${info.entry}"></script></body></html>\n`
  if (html !== expectedHtml) fail('HTML contains unexpected markup or resources')
  const manifestCss = [...new Set(records.flatMap(record => [record.file, ...(record.css || [])]).filter(name => name?.endsWith('.css')))].sort()
  compareRecords(css, manifestCss, 'manifest CSS references')
  const cssResources = new Set()
  for (const stylesheet of css) {
    if (!literalPath(stylesheet)) fail('unsafe stylesheet path')
    const bytes = readFileSync(path.join(root, stylesheet), 'utf8')
    if (!bytes.trim() || /@import\b/i.test(bytes)) fail('CSS is empty or imports untracked resources')
    for (const [, resource] of bytes.matchAll(/url\(\s*["']?([^)'"\s]+)["']?\s*\)/g)) {
      if (resource.startsWith('data:')) continue
      const resolved = path.posix.normalize(path.posix.join(path.posix.dirname(stylesheet), resource))
      if (!literalPath(resolved) || !files.some(file => file.path === resolved)) fail('CSS resource is missing or external')
      cssResources.add(resolved)
    }
  }
  if (files.some(file => /\.(?:mjs|cjs|map)$/.test(file.path) || /iframe|micro-sdk|config\.js|vue\.global/i.test(file.path)) ||
    files.filter(file => file.path.endsWith('.js')).some(file => file.path !== info.entry)) fail('unexpected legacy or split JavaScript asset')
  const allowedFiles = new Set(['index.html', 'build-info.json', 'asset-manifest.json', 'assets/vue.LICENSE.txt', releaseHashFile, info.entry, ...css, ...cssResources])
  if (files.some(file => !allowedFiles.has(file.path))) fail('release contains an untracked asset')
  const javascript = readFileSync(path.join(root, info.entry), 'utf8')
  if (/\bimport\s*\(|iframe-bridge|window\.Vue|postMessage\s*\(/.test(javascript) || !javascript.includes('3.0.0') ||
    !javascript.includes('UNSUPPORTED_SDK_VERSION')) fail('UMD contains legacy transport or lacks the SDK 3 runtime')
  const containsLiteral = value => javascript.includes(JSON.stringify(value)) ||
    javascript.includes('`' + value.replaceAll('\\', '\\\\').replaceAll('`', '\\`').replaceAll('${', '\\${') + '`') ||
    javascript.includes("'" + value.replaceAll('\\', '\\\\').replaceAll("'", "\\'") + "'")
  if (![config.version, config.apiBase, ...config.allowedHostOrigins].every(containsLiteral)) fail('UMD public configuration differs from metadata')
  await checkLifecycle(javascript, info)
  return { application: info.application, version: info.version, runtime: info.runtime, sdkVersion: info.sdkVersion,
    directory: root, entry: info.entry, sourceSha256: info.sourceSha256, inventorySha256: hash(readFileSync(path.join(root, releaseHashFile))), files: files.length }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (process.argv.length > 3) throw new Error('Usage: npm run release:check -- [release-directory]')
  console.log(JSON.stringify(await checkRelease(process.argv[2]), null, 2))
}
