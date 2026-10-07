// Opt-in Phase11 acceptance: actual Rust main, independent Node backend, two
// owned PostgreSQL databases, actual notification service, Nginx and Chromium.
// No JWT is minted here. SQL prepares platform identities and authorization;
// every work order and every notification is created through the real APIs.
import assert from 'node:assert/strict'
import test from 'node:test'
import { randomBytes, randomUUID, createHash } from 'node:crypto'
import { spawn } from 'node:child_process'
import { createServer } from 'node:http'
import { createRequire } from 'node:module'
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, readlinkSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const workspace = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const dependencies = createRequire(path.join(workspace, 'mini-services/notification-ws/package.json'))
const APP = 'work-orders'
const API = '/work-orders-api/tickets'
const PREFIX = 'work_orders_platform_e2e_'
const TEMP_PREFIX = 'dante-work-orders-platform-e2e-'
const platformAdminUrl = process.env.WORK_ORDERS_E2E_PLATFORM_DATABASE_URL ?? process.env.TEST_DATABASE_URL
const businessAdminUrl = process.env.WORK_ORDERS_E2E_BUSINESS_DATABASE_URL ?? platformAdminUrl
const binaryArgument = process.env.WORK_ORDERS_E2E_BACKEND_BINARY
const requirements = {
  WORK_ORDERS_E2E_BACKEND_BINARY: binaryArgument,
  WORK_ORDERS_E2E_PLATFORM_DATABASE_URL: platformAdminUrl,
  NGINX_BIN: process.env.NGINX_BIN,
  WORK_ORDERS_E2E_HOST_DIST: process.env.WORK_ORDERS_E2E_HOST_DIST,
  PLAYWRIGHT_MODULE: process.env.PLAYWRIGHT_MODULE,
  BROWSER_EXECUTABLE: process.env.BROWSER_EXECUTABLE,
}
const missing = Object.entries(requirements).filter(([, value]) => !value).map(([name]) => name)
const scenario=process.env.WORK_ORDERS_E2E_SCENARIO ?? 'full'
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
const sorted = (rows) => rows.map((row) => row.id).sort()
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex')
const scopes = {
  SELF: ['ownRoot', 'ownChild'],
  ORG: ['ownRoot', 'otherRoot'],
  ORG_SUBTREE: ['ownRoot', 'otherRoot', 'ownChild', 'otherChild'],
  TENANT: ['ownRoot', 'otherRoot', 'ownChild', 'otherChild', 'peer'],
}

function safeCode(error) {
  const code = error?.code
  return typeof code === 'string' && /^[A-Z0-9_]{2,32}$/.test(code) ? code : 'UNAVAILABLE'
}
async function database(operation, callback) {
  try { return await callback() } catch (error) { throw new Error(`Work-order real E2E ${operation} failed (${safeCode(error)})`) }
}
const query = (client, sql, values = []) => database('database query', () => client.query(sql, values))
function ownedUrl(source, name) {
  try {
    const url = new URL(source)
    if (!['postgres:', 'postgresql:'].includes(url.protocol)) throw new Error()
    url.pathname = `/${name}`
    for (const key of ['database', 'dbname', 'application_name']) url.searchParams.delete(key)
    return url.toString()
  } catch { throw new Error('Real E2E administration URL must use PostgreSQL') }
}
async function until(predicate, message, timeout = 10000) {
  const end = Date.now() + timeout
  while (!await predicate()) {
    assert.ok(Date.now() < end, message)
    await delay(50)
  }
}
async function port() {
  const server = createServer()
  await new Promise((resolve, reject) => {
    server.once('error', () => reject(new Error('Linux IPv6 loopback is required for real E2E')))
    server.listen(0, '::1', resolve)
  })
  const value = server.address().port
  await new Promise((resolve) => server.close(resolve))
  return value
}
function inventory(root) {
  const files = {}
  function visit(directory) {
    for (const name of readdirSync(directory).sort()) {
      const absolute = path.join(directory, name)
      const stat = lstatSync(absolute)
      assert.ok(!stat.isSymbolicLink(), 'production host and owned release trees must not contain links')
      if (stat.isDirectory()) visit(absolute)
      else if (stat.isFile()) files[path.relative(root, absolute)] = digest(readFileSync(absolute))
    }
  }
  visit(root)
  return files
}
function processIdentity(pid) {
  try {
    const directory=`/proc/${pid}`
    const value=readFileSync(`${directory}/stat`,'utf8')
    const fields=value.slice(value.lastIndexOf(')')+2).split(' ')
    return {pid:Number(pid),parentPid:Number(fields[1]),startTime:fields[19],exe:readlinkSync(`${directory}/exe`),cwd:readlinkSync(`${directory}/cwd`)}
  } catch { return null }
}
async function fixture(t) {
  assert.equal(process.platform, 'linux', 'Run the entire fixture in Linux so directly owned service PIDs are tracked')
  const { default: pg } = await import(pathToFileURL(dependencies.resolve('pg')).href)
  const { io: createClient } = dependencies('socket.io-client')
  const { loadConfig } = await import('../../../mini-services/notification-ws/config.mjs')
  const { createPostgresStore } = await import('../../../mini-services/notification-ws/postgres-store.mjs')
  const { createNotificationService } = await import('../../../mini-services/notification-ws/service.mjs')
  const { chromium } = dependencies(path.resolve(process.env.PLAYWRIGHT_MODULE))
  const binary = realpathSync(path.resolve(binaryArgument))
  const nginx = realpathSync(path.resolve(process.env.NGINX_BIN))
  const browserExecutable = realpathSync(path.resolve(process.env.BROWSER_EXECUTABLE))
  const hostRoot = realpathSync(path.resolve(process.env.WORK_ORDERS_E2E_HOST_DIST))
  const hostHtml = readFileSync(path.join(hostRoot, 'index.html'), 'utf8')
  assert.ok(hostHtml.includes('<div id="root">') || hostHtml.includes('<div id="root"></div>'), 'host dist must contain the real production application entry')
  assert.ok(!hostHtml.includes('/@vite/client') && !hostHtml.includes('/src/main'), 'production-host proof must not use a Vite development server')
  assert.ok(/assets\/.+\.js/.test(hostHtml), 'production host must reference its built JavaScript')
  const hostHashes = inventory(hostRoot)
  const tempBase = realpathSync(os.tmpdir())
  const root = realpathSync(mkdtempSync(path.join(tempBase, TEMP_PREFIX)))
  const expectedRoot = root
  const releaseRoot = path.join(root, 'micro-apps')
  const logs = []
  const secrets = new Set([platformAdminUrl, businessAdminUrl])
  const processes = []
  const sockets = []
  const databases = []
  let notificationService
  let browser
  let browserIdentity
  let qaRoot,qaParent,debugDeadline
  const jwtSecret = randomBytes(48).toString('hex')
  const clientSecret = randomBytes(40).toString('hex')
  secrets.add(jwtSecret); secrets.add(clientSecret)
  secrets.add(Buffer.from(`work-orders-client:${clientSecret}`).toString('base64'))
  function saveOwnership() {
    if (!existsSync(root)) return
    writeFileSync(path.join(root,'ownership.json'),JSON.stringify({schemaVersion:'1.0',root,tempBase,
      harness:processIdentity(process.pid),databases:databases.map((item) => ({name:item.name,expectedName:item.expectedName,created:item.created})),
      processes:processes.map((item) => ({label:item.label,identity:item.identity,closed:item.closed})),
      browser:browserIdentity ? {...browserIdentity,closed:!processIdentity(browserIdentity.pid)} : null,
      visualQaRoot:qaRoot ?? null,
    })+'\n',{mode:0o600})
  }
  saveOwnership()

  function launch(label, executable, args, options = {}) {
    const child = spawn(executable, args, { windowsHide: true, cwd: root, stdio: ['ignore', 'pipe', 'pipe'], ...options })
    const item = { label, child, identity:processIdentity(child.pid), closed: false, error: null, stdout: [], stderr: [] }
    item.exited = new Promise((resolve) => child.once('close', () => { item.closed = true; saveOwnership(); resolve() }))
    child.on('error', (error) => { item.error = safeCode(error) })
    child.stdout.on('data', (bytes) => { item.stdout.push(bytes.toString()); logs.push(bytes.toString()) })
    child.stderr.on('data', (bytes) => { item.stderr.push(bytes.toString()); logs.push(bytes.toString()) })
    processes.push(item)
    saveOwnership()
    return item
  }
  async function stop(item) {
    if (item.closed) return
    item.child.kill(item.label === 'nginx' ? 'SIGQUIT' : 'SIGTERM')
    await Promise.race([item.exited, delay(5000)])
    if (!item.closed) { item.child.kill('SIGKILL'); await Promise.race([item.exited, delay(3000)]) }
    assert.ok(item.closed, 'owned Linux service exit must be confirmed before cleanup')
  }
  t.after(async () => {
    const errors = []
    let browserStopped = true, notificationStopped = true
    for (const socket of sockets) socket.disconnect()
    try { await browser?.close() } catch { browserStopped = false; errors.push('owned browser stop failed') }
    try { await notificationService?.stop() } catch { notificationStopped = false; errors.push('owned notification service stop failed') }
    for (const item of [...processes].reverse()) {
      try { await stop(item) } catch { errors.push(`owned ${item.label} stop failed`) }
    }
    const allStopped = browserStopped && notificationStopped && processes.every((item) => item.closed)
    for (const item of databases) {
      try { await item.writer?.end() } catch { errors.push('owned database writer close failed') }
      try {
        if (item.created && item.connected && allStopped) {
          assert.equal(item.name, item.expectedName)
          assert.match(item.name, /^work_orders_platform_e2e_[pb]_[0-9a-f]{32}$/)
          assert.ok(item.name.startsWith(PREFIX))
          await query(item.admin, `DROP DATABASE "${item.name}" WITH (FORCE)`)
          item.created = false
          saveOwnership()
        }
      } catch { errors.push('owned database cleanup failed') }
      finally { try { await item.admin?.end() } catch { errors.push('owned administration connection close failed') } }
    }
    if (allStopped) {
      if(qaRoot) {
        const relativeQa=path.relative(qaParent,qaRoot)
        assert.ok(relativeQa && !relativeQa.startsWith('..') && !path.isAbsolute(relativeQa))
        assert.equal(path.dirname(qaRoot),qaParent)
        assert.equal(path.basename(qaRoot),`qa-${path.basename(expectedRoot)}`)
        assert.ok(!lstatSync(qaRoot).isSymbolicLink())
        assert.equal(JSON.parse(readFileSync(path.join(qaRoot,'ownership.json'),'utf8')).ownerRoot,expectedRoot)
        // Opt-in visual artifacts survive service/database teardown. Reviewers
        // explicitly acknowledge this exact owned directory before removing it;
        // a timeout must never silently discard acceptance screenshots.
        console.log(`[Phase11 visual QA retained directory] ${imageDisplayPath(qaRoot)}`)
      }
      const absolute = path.resolve(root)
      const relative = path.relative(tempBase, absolute)
      assert.equal(absolute, expectedRoot)
      assert.ok(relative && !relative.startsWith('..') && !path.isAbsolute(relative))
      assert.equal(path.dirname(absolute), tempBase)
      assert.ok(path.basename(absolute).startsWith(TEMP_PREFIX))
      assert.ok(!lstatSync(absolute).isSymbolicLink())
      rmSync(absolute, { recursive: true, force: true })
    } else errors.push('service exit unconfirmed; owned databases and files retained')
    if(scenario==='full') assert.deepEqual(inventory(hostRoot), hostHashes, 'independent releases must preserve every production host file byte')
    const captured = [logs.join(''), ...processes.flatMap((item) => [item.stdout.join(''),item.stderr.join('')])].join('\n')
    for (const value of secrets) if (value) assert.ok(!captured.includes(value), 'service/build/browser logs must not disclose connection URLs, client secret, signing key, tickets or Bearers')
    assert.equal(errors.length, 0, errors.join('; '))
  })
  async function createDatabase(source, kind) {
    const name = `${PREFIX}${kind}_${randomUUID().replaceAll('-', '')}`
    const connectionString = ownedUrl(source, name)
    secrets.add(connectionString)
    const item = { name, expectedName: name, created: false, connected: false }
    databases.push(item)
    saveOwnership()
    item.admin = new pg.Client({ connectionString: source, application_name: 'work-orders-e2e-admin', connectionTimeoutMillis: 10000, query_timeout: 15000 })
    item.admin.on('error', () => logs.push('owned test administration connection unavailable'))
    await database('administration connect', () => item.admin.connect())
    item.connected = true
    await query(item.admin, `CREATE DATABASE "${name}" TEMPLATE template0`)
    item.created = true
    saveOwnership()
    item.writer = new pg.Client({ connectionString, application_name: 'work-orders-e2e-writer', connectionTimeoutMillis: 10000, query_timeout: 15000 })
    item.writer.on('error', () => logs.push('owned test writer connection unavailable'))
    await database('writer connect', () => item.writer.connect())
    assert.equal((await query(item.writer, 'SELECT current_database() AS name')).rows[0].name, name)
    return { ...item, connectionString }
  }
  const platformDb = await createDatabase(platformAdminUrl, 'p')
  const businessDb = await createDatabase(businessAdminUrl, 'b')
  assert.notEqual(platformDb.name, businessDb.name)
  const platformPort = await port(), businessPort = await port(), gatewayPort = await port()
  const platformBase = `http://[::1]:${platformPort}`
  const businessBase = `http://[::1]:${businessPort}`
  const gatewayBase = `http://[::1]:${gatewayPort}`
  // main uses dotenv_override: this empty owned file prevents real project or
  // ancestor configuration from overriding the explicitly isolated resources.
  writeFileSync(path.join(root, '.env'), '')
  const platform = launch('platform', binary, [], { env: {
    ...process.env, DATABASE_URL: platformDb.connectionString, JWT_SECRET: jwtSecret,
    BIND_ADDR: `[::1]:${platformPort}`, PUBLIC_APP_URL: gatewayBase,
    MICRO_GATEWAY_ORIGIN: gatewayBase, MICRO_ALLOWED_ORIGINS: gatewayBase,
    APPLICATION_ENVIRONMENT: 'development', AUTO_CREATE_DATABASE: 'false',
    STATIC_DIR: '', SESSION_TTL_HOURS: '1', ACCESS_TOKEN_TTL_SECONDS: '300', RUST_LOG: 'warn',
  } })
  async function http(base, method, endpoint, token, body, { headers = {}, timeout = 10000 } = {}) {
    if (token) headers = { ...headers, Authorization: `Bearer ${token}` }
    if (body !== undefined) headers = { ...headers, 'Content-Type': 'application/json' }
    let response
    try { response = await fetch(new URL(endpoint, base), { method, headers, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(timeout), redirect: 'error' }) }
    catch (error) { throw new Error(`Real E2E HTTP request failed (${safeCode(error)})`) }
    const text = await response.text()
    let data
    try { data = JSON.parse(text) } catch { data = null }
    return { status: response.status, data, text, headers: response.headers }
  }
  async function platformOk(method, endpoint, token, body, expected = 200) {
    const response = await http(platformBase, method, endpoint, token, body)
    assert.equal(response.status, expected, 'real platform API status differs from the frozen contract')
    return response.data
  }
  await until(async () => {
    assert.ok(!platform.error && !platform.closed, 'fresh compiled platform exited before readiness')
    try { return (await http(platformBase, 'GET', '/api/health', null, undefined, { timeout: 1000 })).status === 200 } catch { return false }
  }, 'actual platform main migrations and seed must complete', 60000)
  assert.deepEqual((await query(platformDb.writer, 'SELECT success FROM _sqlx_migrations WHERE version=22')).rows, [{ success: true }], 'fresh real platform binary must contain the persisted-notification migration')
  async function login(username, organizationId) {
    const response = await platformOk('POST', '/api/auth/login', null, { username, password: 'admin123' })
    secrets.add(response.ticket)
    assert.ok(response.memberships.some((membership) => membership.organizationId === organizationId), 'context entry requires a real direct membership')
    const context = await platformOk('POST', '/api/auth/context', response.ticket, { organizationId })
    secrets.add(context.accessToken)
    const socketContext = await platformOk('GET', '/api/notifications/socket-context', context.accessToken)
    return { session: context.accessToken, socketContext, userId: context.user.id, tenantId: context.tenant.id, organizationId: context.organization.id, username }
  }
  async function appToken(actor) {
    if (!actor.token || Date.now() - actor.issued > 230000) {
      const issued = await platformOk('POST', '/api/oauth/session-token', actor.session, { appCode: APP }, 201)
      assert.equal(issued.audience, APP)
      actor.token = issued.accessToken; actor.issued = Date.now(); secrets.add(actor.token)
    }
    return actor.token
  }
  const seeded = await query(platformDb.writer, "SELECT m.organization_id FROM memberships m JOIN users u ON u.id=m.user_id WHERE u.username='admin' LIMIT 1")
  const administrator = await login('admin', seeded.rows[0].organization_id)
  const { run: runAdmin } = await import('../deploy/platform-admin-core.mjs')
  const adminEnvironment = { PLATFORM_API_ORIGIN: platformBase, PLATFORM_ADMIN_BEARER: administrator.session, PLATFORM_CLIENT_ID: 'work-orders-client' }
  const adminCommand = (args) => runAdmin([...args, '--environment', 'development', '--allow-loopback-http'], adminEnvironment)
  const ids = await platformFixtures(platformDb.writer, clientSecret, adminCommand)
  const inspect = await adminCommand(['inspect'])
  assert.equal(inspect.applicationId,ids.application); assert.equal(inspect.clientAssociated,true); assert.deepEqual(inspect.missingMenus,[])
  const repeatBootstrap = await adminCommand(['bootstrap','--apply'])
  assert.deepEqual(repeatBootstrap.created,[], 'repeating actual bootstrap must be idempotent and must not grant access')
  assert.deepEqual(repeatBootstrap.grants,[])
  const actors = {}
  for (const [key, username, organization] of [
    ['root', 'p11-owner', 'a-branch'], ['other', 'p11-other', 'a-branch'],
    ['ownChild', 'p11-owner', 'a-child'], ['child', 'p11-child', 'a-child'],
    ['peer', 'p11-peer', 'a-peer'], ['ownRemote', 'p11-owner', 'b-branch'],
    ['remote', 'p11-remote', 'b-branch'], ['remoteChild', 'p11-remote-child', 'b-child'], ['inherited', 'p11-inherited', 'a-branch'],
  ]) actors[key] = await login(username, ids.organizations[organization].id)
  const business = launch('business', process.execPath, [path.join(workspace, 'business-apps/work-orders/backend/index.mjs')], { env: {
    ...process.env, WORK_ORDERS_DATABASE_URL: businessDb.connectionString,
    PLATFORM_API_ORIGIN: platformBase, PLATFORM_CLIENT_ID: 'work-orders-client', PLATFORM_CLIENT_SECRET: clientSecret,
    NODE_ENV: 'test', WORK_ORDERS_HOST: '::1', WORK_ORDERS_PORT: String(businessPort), PLATFORM_AUTH_TIMEOUT_MS: '3000',
    WORK_ORDERS_ALLOWED_ORIGINS: 'null',
  } })
  await until(async () => {
    assert.ok(!business.error && !business.closed, 'actual Node business main exited before readiness')
    try { return (await http(businessBase, 'GET', API)).status === 401 } catch { return false }
  }, 'independent business migration and listener must become ready', 45000)
  assert.equal((await query(businessDb.writer, "SELECT to_regclass('work_orders')::text AS name")).rows[0].name, 'work_orders')
  assert.equal((await query(businessDb.writer, "SELECT to_regclass('users')::text AS name")).rows[0].name, null, 'business database must not contain platform identity tables')
  assert.equal((await query(platformDb.writer, "SELECT to_regclass('work_orders')::text AS name")).rows[0].name, null, 'platform database must not contain business tickets')
  const capture = (...values) => logs.push(JSON.stringify(values, (_, value) => value instanceof Error ? { code: safeCode(value) } : value))
  const logger = { info: capture, warn: capture, error: capture, debug: capture, log: capture }
  const config = { ...loadConfig({ DATABASE_URL: platformDb.connectionString, PLATFORM_API_BASE_URL: platformBase, NOTIFICATION_ALLOWED_ORIGINS: gatewayBase }), host: '::1', port: 0, recheckMs: 1000, healthProbeMs: 30000 }
  notificationService = createNotificationService(config, { store: createPostgresStore(config, logger), logger })
  const notificationAddress = await notificationService.start()
  await until(async () => (await notificationService.probeHealth()).status === 'UP', 'notification service must use the real platform and PostgreSQL')

  async function command(label, args) {
    const item = launch(label, process.execPath, args)
    await Promise.race([item.exited, delay(60000)])
    assert.ok(item.closed && !item.error && item.child.exitCode === 0, 'actual frontend build and packager must complete successfully')
  }
  const manifests = {}, releaseFiles = {}
  for (const version of ['1.0.0', '1.0.1']) {
    const output = path.join(root, `vue-${version}`)
    await command('vue-build', [path.join(workspace, 'business-apps/work-orders/frontend/scripts/build.mjs'), '--out-dir', output, '--version', version, '--allowed-host-origin', gatewayBase])
    await command('packager', [path.join(workspace, 'scripts/package-microapp.mjs'), '--app', APP, '--environment', 'development', '--version', version, '--input', output, '--output-root', releaseRoot])
    const packedRoot = path.join(releaseRoot, APP, 'versions', version)
    releaseFiles[version] = path.join(packedRoot,'release.json')
    manifests[version] = JSON.parse(readFileSync(releaseFiles[version], 'utf8'))
    assert.equal(manifests[version].entryUrl, `/sub-apps/${APP}/versions/${version}/index.html`)
    assert.ok(manifests[version].resources.some((resource) => /vue/i.test(resource.url)), 'actual independent Vue vendor must be packaged')
    for (const file of Object.keys(inventory(packedRoot))) {
      const contents = readFileSync(path.join(packedRoot, file), 'utf8')
      for (const secret of secrets) assert.ok(!contents.includes(secret), 'published frontend resources must not contain any runtime secret')
    }
  }
  assert.notDeepEqual(inventory(path.join(releaseRoot, APP, 'versions/1.0.0')), inventory(path.join(releaseRoot, APP, 'versions/1.0.1')), 'two actual Vue builds must have different resource bytes')
  assert.notEqual(digest(readFileSync(path.join(releaseRoot,APP,'versions/1.0.0/assets/config.js'))),
    digest(readFileSync(path.join(releaseRoot,APP,'versions/1.0.1/assets/config.js'))), 'executable version configuration must differ in the actual two Vue builds')
  const quoted = (value) => `"${value.replaceAll('\\', '/')}"`
  const mime = process.env.NGINX_MIME_TYPES ?? [path.resolve(path.dirname(nginx), '../conf/mime.types'), path.resolve(path.dirname(nginx), '../../conf/mime.types')].find(existsSync)
  assert.ok(mime && existsSync(mime), 'supply NGINX_MIME_TYPES from the local Nginx distribution')
  let nginxConfig = readFileSync(path.join(workspace, 'deploy/nginx.conf'), 'utf8')
  for (const [source, replacement] of [
    ['worker_processes auto;', 'worker_processes 1;'], ['include mime.types;', `include ${quoted(mime)};`],
    ['listen 80;', `listen [::1]:${gatewayPort};`],
    ['/srv/dante/frontend/dist', hostRoot], ['/srv/dante/micro-apps', releaseRoot],
    ['http://127.0.0.1:3001', platformBase], ['http://127.0.0.1:3004', businessBase],
    ['http://127.0.0.1:3003', `http://[::1]:${notificationAddress.port}`],
  ]) { assert.ok(nginxConfig.includes(source), 'actual Nginx template must contain the expected deployment configuration'); nginxConfig = nginxConfig.replaceAll(source, replacement) }
  nginxConfig = `pid ${quoted(path.join(root, 'nginx.pid'))};\nerror_log stderr warn;\n${nginxConfig.replace('http {', 'http {\n    access_log off;')}`
  const configPath = path.join(root, 'nginx.conf')
  writeFileSync(configPath, nginxConfig)
  const gateway = launch('nginx', nginx, ['-p', `${root}/`, '-c', configPath, '-g', 'daemon off;'])
  await until(async () => {
    assert.ok(!gateway.error && !gateway.closed, 'owned Nginx exited before readiness')
    try { return (await http(gatewayBase, 'GET', '/')).status === 200 } catch { return false }
  }, 'actual Nginx must serve the production host and real upstream services')
  assert.equal((await http(gatewayBase, 'GET', '/')).text, hostHtml)
  assert.equal((await http(gatewayBase, 'GET', '/@vite/client')).text, hostHtml, 'SPA fallback does not supply a Vite development client')
  assert.equal((await http(gatewayBase, 'GET', '/work-orders-api/missing', await appToken(actors.root))).status, 404, 'business prefix must never fall back to host HTML')
  const preflight = await http(gatewayBase, 'OPTIONS', API, null, undefined, { headers: { Origin: 'null', 'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'authorization,content-type' } })
  assert.equal(preflight.status, 204)
  assert.equal(preflight.headers.get('access-control-allow-origin'), 'null')
  assert.match(preflight.headers.get('access-control-allow-headers'), /authorization/i)
  assert.equal(preflight.headers.get('access-control-allow-credentials'), null)

  // The real introspection endpoint allows 30 requests/minute per IP. This
  // queue only schedules actual requests; it never substitutes an API response.
  let scheduled = 0
  async function pace() {
    const slot = Math.max(Date.now(), scheduled)
    scheduled = slot + 2150
    await delay(Math.max(0, slot - Date.now()))
  }
  async function businessCall(actor, method, endpoint = API, body, options = {}) {
    if (!options.unpaced) await pace()
    const token = typeof actor === 'string' ? actor : actor ? await appToken(actor) : undefined
    return http(gatewayBase, method, endpoint, token, body, options)
  }
  async function accepted(actor, method, endpoint = API, body, expected = 200) {
    const response = await businessCall(actor, method, endpoint, body)
    assert.equal(response.status, expected, 'real business API status differs from the frozen contract')
    return response.data
  }
  async function denied(actor, method, endpoint = API, body, expected = 404) {
    const response = await businessCall(actor, method, endpoint, body)
    assert.equal(response.status, expected, 'real authorization must reject the attempted operation')
    return response
  }
  const create = (actor, title, extras = {}) => accepted(actor, 'POST', API, { title, ...extras }, 201)
  const records = async (actor, filter = '') => {
    const page = await accepted(actor, 'GET', `${API}?page=1&size=100${filter}`)
    assert.ok(Array.isArray(page.records)); assert.equal(typeof page.total, 'number')
    return page.records
  }
  async function setGrants(actor, scope, actions = ['read', 'create', 'update', 'delete', 'export']) {
    await query(platformDb.writer, `UPDATE membership_data_grants g SET scope_type=$1,updated_at=now()
      FROM memberships m WHERE m.id=g.membership_id AND m.user_id=$2 AND m.organization_id=$3
      AND g.application_id=$4 AND g.resource_code='ticket' AND g.action_code=ANY($5::text[])`,
    [scope, actor.userId, actor.organizationId, ids.application, actions])
  }
  async function connectSocket(actor) {
    const socket = createClient(gatewayBase, { path: '/socket.io/', transports: ['websocket'], extraHeaders: { Origin: gatewayBase }, reconnection: false, forceNew: true, autoConnect: false })
    sockets.push(socket)
    const events = [], authenticated = [], denied = [], errors = []
    socket.on('notification', (packet) => events.push(packet))
    socket.on('authenticated', (packet) => authenticated.push(packet))
    socket.on('unauthorized', (packet) => denied.push(packet))
    socket.on('connect_error', () => errors.push('connection failed'))
    socket.connect()
    await until(() => socket.connected || errors.length, 'real Socket.IO gateway transport must connect')
    assert.ok(socket.connected)
    socket.emit('authenticate', { token: actor.session })
    await until(() => authenticated.length || denied.length, 'real platform must authenticate the socket')
    assert.equal(denied.length, 0)
    assert.equal(authenticated[0].organizationId, actor.organizationId)
    return { socket, events }
  }
  async function launchBrowser() {
    if(browser?.isConnected()) return browser
    const configHome=path.join(root,'xdg-config'),cacheHome=path.join(root,'xdg-cache')
    mkdirSync(configHome,{recursive:true}); mkdirSync(cacheHome,{recursive:true})
    const browserEnvironment={...process.env,XDG_CONFIG_HOME:configHome,XDG_CACHE_HOME:cacheHome,TMPDIR:root}
    if (process.env.WORK_ORDERS_E2E_FONT_DIR) {
      const fontDirectory=realpathSync(path.resolve(process.env.WORK_ORDERS_E2E_FONT_DIR))
      assert.ok(lstatSync(fontDirectory).isDirectory(),'visual QA font directory must be a local runtime directory')
      const fontCache=path.join(root,'font-cache'),fontFile=path.join(root,'fonts.conf')
      mkdirSync(fontCache,{recursive:true})
      const xml=(value) => value.replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('>','&gt;')
      writeFileSync(fontFile,`<?xml version="1.0"?><fontconfig><dir>${xml(fontDirectory)}</dir><cachedir>${xml(fontCache)}</cachedir><alias><family>sans-serif</family><prefer><family>Microsoft YaHei</family></prefer></alias><alias><family>system-ui</family><prefer><family>Microsoft YaHei</family></prefer></alias></fontconfig>`)
      browserEnvironment.FONTCONFIG_FILE=fontFile
    }
    browser = await chromium.launch({ executablePath: browserExecutable, headless: true, downloadsPath: path.join(root,'downloads'),
      env:browserEnvironment,args: ['--no-proxy-server'] })
    const ownedBrowsers=readdirSync('/proc').filter((value) => /^\d+$/.test(value)).map(processIdentity)
      .filter((item) => item?.parentPid===process.pid && item.exe===browserExecutable)
    assert.equal(ownedBrowsers.length,1,'actual Chromium must be a directly owned child with an observable PID')
    browserIdentity=ownedBrowsers[0]; saveOwnership()
    return browser
  }
  function qaImage(name) {
    assert.match(name,/^[a-z0-9-]+$/)
    if(!qaRoot) {
      qaParent=realpathSync(path.join(workspace,'target/phase11-runtime'))
      const candidate=path.join(qaParent,`qa-${path.basename(root)}`)
      mkdirSync(candidate) // exclusive: never use or remove an existing directory
      writeFileSync(path.join(candidate,'ownership.json'),JSON.stringify({ownerRoot:root})+'\n',{flag:'wx',mode:0o600})
      qaRoot=realpathSync(candidate);saveOwnership()
    }
    return path.join(qaRoot,`${name}.png`)
  }
  async function visualCheckpoint() {
    if(!qaRoot) return
    console.log(`[Phase11 visual QA ready for review] ${imageDisplayPath(qaRoot)}`)
  }
  async function debugResume() {
    assert.equal(scenario,'browser','long diagnostic hold is forbidden in the complete acceptance gate')
    assert.ok(qaRoot,'diagnostic resume requires an owned QA directory')
    debugDeadline ??= Date.now()+600000
    const deadline=debugDeadline
    while(Date.now()<deadline) {
      const marker=path.join(qaRoot,'resume')
      if(existsSync(marker)) {
        assert.ok(lstatSync(marker).isFile() && !lstatSync(marker).isSymbolicLink())
        rmSync(marker)
        return true
      }
      await delay(250)
    }
    return false
  }
  return { root, ids, actors, administrator, manifests, releaseFiles, adminCommand, gatewayBase, platformBase, platformDb, businessDb, secrets,
    http, platformOk, appToken, login, accepted, denied, businessCall, create, records, setGrants, connectSocket, launchBrowser, qaImage, visualCheckpoint, debugResume, pace, hostHashes, hostRoot, logs }
}

async function platformFixtures(writer, clientSecret, adminCommand) {
  await query(writer, "INSERT INTO tenants(name,code,is_platform) VALUES('Phase11 tenant A','p11-a',false),('Phase11 tenant B','p11-b',false)")
  await query(writer, `INSERT INTO organizations(tenant_id,name,code)
    SELECT id,CASE code WHEN 'p11-a' THEN 'Phase11 A top' ELSE 'Phase11 B top' END,
      CASE code WHEN 'p11-a' THEN 'p11-a-top' ELSE 'p11-b-top' END FROM tenants WHERE code IN ('p11-a','p11-b')`)
  await query(writer, `INSERT INTO organizations(tenant_id,parent_id,name,code)
    SELECT tenant_id,id,CASE code WHEN 'p11-a-top' THEN 'Phase11 A branch' ELSE 'Phase11 B branch' END,
      CASE code WHEN 'p11-a-top' THEN 'p11-a-branch' ELSE 'p11-b-branch' END FROM organizations WHERE code IN ('p11-a-top','p11-b-top')`)
  await query(writer, `INSERT INTO organizations(tenant_id,parent_id,name,code)
    SELECT tenant_id,id,CASE code WHEN 'p11-a-branch' THEN 'Phase11 A child' ELSE 'Phase11 B child' END,
      CASE code WHEN 'p11-a-branch' THEN 'p11-a-child' ELSE 'p11-b-child' END FROM organizations WHERE code IN ('p11-a-branch','p11-b-branch')`)
  await query(writer, "INSERT INTO organizations(tenant_id,parent_id,name,code) SELECT tenant_id,id,'Phase11 A peer','p11-a-peer' FROM organizations WHERE code='p11-a-top'")
  const usernames = ['p11-owner','p11-other','p11-child','p11-peer','p11-remote','p11-remote-child','p11-inherited']
  assert.equal((await query(writer, `INSERT INTO users(username,password_hash,real_name,is_super_admin)
    SELECT fixture,seeded.password_hash,fixture,false FROM users seeded CROSS JOIN unnest($1::text[]) fixture WHERE seeded.username='admin'`, [usernames])).rowCount, usernames.length)
  const memberships = [['p11-owner','p11-a-branch'],['p11-owner','p11-a-child'],['p11-owner','p11-b-branch'],
    ['p11-other','p11-a-branch'],['p11-child','p11-a-child'],['p11-peer','p11-a-peer'],['p11-remote','p11-b-branch'],['p11-remote-child','p11-b-child'],['p11-inherited','p11-a-branch']]
  for (const [username, code] of memberships) await query(writer, `INSERT INTO memberships(user_id,organization_id)
    SELECT u.id,o.id FROM users u CROSS JOIN organizations o WHERE u.username=$1 AND o.code=$2`, [username,code])
  await query(writer, `INSERT INTO oauth_clients(client_id,client_secret_hash,client_name,client_type,grant_types,scopes,redirect_uris,access_token_validity,refresh_token_validity)
    VALUES('work-orders-client',$1,'work-orders-client','CONFIDENTIAL','client_credentials','openid','',300,300)`, [digest(clientSecret)])
  // The operator tool runs its actual native HTTP client against this real main.
  // The existing explicitly selected confidential client is linked by bootstrap.
  const bootstrap = await adminCommand(['bootstrap','--apply'])
  assert.equal(bootstrap.ok,true); assert.equal(bootstrap.releaseActivated,false); assert.deepEqual(bootstrap.grants,[])
  assert.equal(Object.keys(bootstrap.menuIds).length,7)
  const application = bootstrap.applicationId
  await query(writer, "INSERT INTO tenant_applications(tenant_id,application_id) SELECT id,$1 FROM tenants WHERE code IN ('p11-a','p11-b')", [application])
  await query(writer, "INSERT INTO organization_applications(organization_id,application_id) SELECT id,$1 FROM organizations WHERE code LIKE 'p11-%'", [application])
  await query(writer, "INSERT INTO tenant_menus(tenant_id,menu_id) SELECT t.id,m.id FROM tenants t CROSS JOIN menus m WHERE t.code IN ('p11-a','p11-b') AND m.application_id=$1", [application])
  await query(writer, "INSERT INTO organization_menus(organization_id,menu_id) SELECT o.id,m.id FROM organizations o CROSS JOIN menus m WHERE o.code LIKE 'p11-%' AND m.application_id=$1", [application])
  // Functional role is assigned at the ancestor; membership data grants below
  // are explicitly local and therefore cannot be inherited with that role.
  await query(writer, "INSERT INTO roles(tenant_id,organization_id,name,code) SELECT tenant_id,id,'Phase11 inherited operations','p11-operations' FROM organizations WHERE code IN ('p11-a-top','p11-b-top')")
  await query(writer, "INSERT INTO role_menus(role_id,menu_id) SELECT r.id,m.id FROM roles r CROSS JOIN menus m WHERE r.code='p11-operations' AND m.application_id=$1", [application])
  await query(writer, `INSERT INTO user_roles(user_id,role_id,organization_id)
    SELECT DISTINCT m.user_id,r.id,r.organization_id FROM memberships m JOIN organizations o ON o.id=m.organization_id
    JOIN roles r ON r.tenant_id=o.tenant_id AND r.code='p11-operations' JOIN users u ON u.id=m.user_id WHERE u.username=ANY($1::text[])`, [usernames])
  for (const [resource, actions] of [['ticket',['read','create','update','delete','export']],['notification',['publish']]])
    await query(writer, `INSERT INTO membership_data_grants(membership_id,application_id,resource_code,action_code,scope_type)
      SELECT m.id,$1,$2,action,'TENANT' FROM memberships m JOIN users u ON u.id=m.user_id CROSS JOIN unnest($3::text[]) action
      WHERE u.username=ANY($4::text[]) AND u.username<>'p11-inherited'`, [application,resource,actions,usernames])
  const organizations = Object.fromEntries((await query(writer, "SELECT id,code,name,tenant_id FROM organizations WHERE code LIKE 'p11-%'")).rows.map((row) => [row.code.slice(4),row]))
  const users = Object.fromEntries((await query(writer, "SELECT id,username,is_super_admin FROM users WHERE username=ANY($1::text[])", [usernames])).rows.map((row) => [row.username,row]))
  assert.ok(Object.values(users).every((row) => row.is_super_admin === false))
  return { application, organizations, users }
}

test(`Phase11 actual platform + independent business DB + production Vue iframe release E2E${scenario==='browser' ? ' (browser DEBUG only)' : ''}`, {
  skip: missing.length ? `Opt in with ${missing.join(', ')}` : false, timeout: 1200000,
}, async (t) => {
  assert.ok(['full','browser'].includes(scenario),'WORK_ORDERS_E2E_SCENARIO must be full or browser (DEBUG only)')
  const h = await fixture(t)
  console.log('[Phase11] Actual two-database fixture, independent services, production host and two Vue packages ready')
  t.beforeEach((current) => console.log(`[Phase11] ${current.name}`))
  const tickets = {}
  for (const [key, actor] of Object.entries(h.actors)) if (key !== 'inherited') {
    tickets[key === 'root' ? 'ownRoot' : key === 'other' ? 'otherRoot' : key === 'child' ? 'otherChild' : key] = await h.create(actor, `Phase11 ${key} ticket`)
  }
  const root = h.actors.root
  const ticketPath = (row) => `${API}/${row.id}`
  for (const row of Object.values(tickets)) {
    assert.match(row.id, /^[0-9a-f-]{36}$/i); assert.ok(row.number)
    assert.equal(row.status,'OPEN'); assert.equal(row.priority,'NORMAL'); assert.equal(row.version,1)
  }
  if (scenario==='browser') {
    await t.test('DEBUG actual production browser only; not the complete HTTP acceptance gate',async () => checkedBrowserScenario(h,tickets))
    return
  }
  await t.test('ancestor role grants operations but cannot supply membership data scope', async () => {
    const token = await h.appToken(h.actors.inherited)
    const context = await h.platformOk('GET', `/api/apps/${APP}/context`, token)
    assert.ok(context.roles.some((role) => role.code === 'p11-operations'))
    assert.ok(context.permissions.includes('ticket:read'))
    assert.equal(context.dataGrants.length,0)
    await h.denied(h.actors.inherited,'GET',API,undefined,403)
    await h.denied(h.actors.inherited,'POST',API,{title:'Inherited role cannot create'},403)
    await h.denied(root.session,'GET',API,undefined,401)
  })
  await t.test('second tenant parent/child members remain isolated including the same global owner', async () => {
    const foreignRows=await h.records(h.actors.remote)
    assert.deepEqual(sorted(foreignRows),[tickets.ownRemote.id,tickets.remote.id,tickets.remoteChild.id].sort())
    assert.ok(foreignRows.every((row) => row.tenantId===h.actors.remote.tenantId))
    await h.denied(h.actors.remote,'GET',ticketPath(tickets.ownRoot))
    await h.setGrants(h.actors.ownRemote,'SELF')
    assert.deepEqual(sorted(await h.records(h.actors.ownRemote)),[tickets.ownRemote.id])
    await h.denied(h.actors.ownRemote,'GET',ticketPath(tickets.ownChild))
    await h.setGrants(h.actors.ownRemote,'TENANT')
  })
  await t.test('four explicit scopes cover real list, detail, update, delete and CSV', async (scopeTest) => {
    for (const [scope, keys] of Object.entries(scopes)) {
      await scopeTest.test(scope, async () => {
        await h.setGrants(root,scope)
        assert.deepEqual(sorted(await h.records(root)), keys.map((key) => tickets[key].id).sort())
        const exported = await h.businessCall(root,'GET',`${API}/export.csv`)
        assert.equal(exported.status,200); assert.match(exported.headers.get('content-type'),/text\/csv/i)
        for (const [key,row] of Object.entries(tickets)) {
          assert.equal(exported.text.includes(row.title),keys.includes(key), 'export must use its own current grant scope')
          if (keys.includes(key)) {
            assert.equal((await h.accepted(root,'GET',ticketPath(row))).id,row.id)
            const changed = await h.accepted(root,'PATCH',ticketPath(row),{version:row.version,description:`Updated through ${scope}`})
            assert.equal(changed.version,row.version+1)
            assert.equal(changed.tenantId,row.tenantId); assert.equal(changed.organizationId,row.organizationId); assert.equal(changed.ownerUserId,row.ownerUserId)
            tickets[key] = changed
          } else {
            const invisible = await h.denied(root,'GET',ticketPath(row))
            assert.ok(!invisible.text.includes(row.title) && !invisible.text.includes(row.id), 'denied resource errors must not reveal the other record')
            await h.denied(root,'PATCH',ticketPath(row),{version:row.version,title:'Forbidden write'})
            await h.denied(root,'DELETE',ticketPath(row),{version:row.version})
          }
        }
        const disposable = await h.create(root,`Soft delete ${scope}`)
        await h.accepted(root,'DELETE',ticketPath(disposable),{version:disposable.version})
        await h.denied(root,'GET',ticketPath(disposable))
        await h.denied(root,'PATCH',ticketPath(disposable),{version:disposable.version,title:'Restore deleted'})
        const persisted = await query(h.businessDb.writer,'SELECT deleted_at FROM work_orders WHERE id=$1',[disposable.id])
        assert.equal(persisted.rowCount,1); assert.ok(persisted.rows[0].deleted_at)
        assert.ok(!(await h.records(root)).some((row) => row.id === disposable.id))
        assert.ok(!(await h.businessCall(root,'GET',`${API}/export.csv`)).text.includes(disposable.title))
        const representative = { SELF:h.actors.ownChild, ORG:h.actors.other, ORG_SUBTREE:h.actors.child, TENANT:h.actors.peer }[scope]
        const distantDelete = await h.create(representative,`Authorized scope deletion ${scope}`)
        await h.accepted(root,'DELETE',ticketPath(distantDelete),{version:distantDelete.version})
        assert.ok((await query(h.businessDb.writer,'SELECT deleted_at FROM work_orders WHERE id=$1',[distantDelete.id])).rows[0].deleted_at)
      })
    }
  })
  await t.test('assignee is a valid direct member and never expands SELF ownership', async () => {
    await h.setGrants(root,'TENANT')
    const assigned = await h.accepted(h.actors.other,'PATCH',ticketPath(tickets.otherRoot),{version:tickets.otherRoot.version,assigneeUserId:root.userId})
    tickets.otherRoot = assigned
    assert.equal(assigned.assigneeUserId,root.userId)
    await h.setGrants(root,'SELF')
    await h.denied(root,'GET',ticketPath(assigned))
    await h.denied(root,'PATCH',ticketPath(assigned),{version:assigned.version,title:'Assignee cannot use SELF'})
    const directory = await h.accepted(root,'GET',`/work-orders-api/assignees?action=create&organizationId=${root.organizationId}&page=1&size=50`)
    assert.ok(directory.records.some((member) => member.userId === h.actors.other.userId))
    await h.denied(root,'POST',API,{title:'Cross organization assignee',assigneeUserId:h.actors.child.userId},404)
    await h.denied(root,'POST',API,{title:'Cross tenant assignee',assigneeUserId:h.actors.remote.userId},404)
    await h.setGrants(root,'TENANT')
  })
  await t.test('subtree authorization excludes disabled descendants', async () => {
    await h.setGrants(root,'ORG_SUBTREE')
    await query(h.platformDb.writer,"UPDATE organizations SET status='DISABLED' WHERE id=$1",[h.actors.child.organizationId])
    try {
      const context = await h.platformOk('GET',`/api/apps/${APP}/context`,await h.appToken(root))
      assert.ok(context.dataGrants.filter((grant) => grant.resource==='ticket').every((grant) => !grant.organizationIds.includes(h.actors.child.organizationId)))
      await h.denied(root,'GET',ticketPath(tickets.otherChild))
      assert.ok(!(await h.records(root)).some((row) => row.organizationId===h.actors.child.organizationId))
    } finally {
      await query(h.platformDb.writer,"UPDATE organizations SET status='ENABLED' WHERE id=$1",[h.actors.child.organizationId])
      await h.setGrants(root,'TENANT')
    }
  })
  await t.test('server-owned fields, version conflicts, workflow and filters remain persistent', async () => {
    for (const fields of [{tenantId:h.actors.remote.tenantId},{organizationId:h.actors.child.organizationId},{ownerUserId:h.actors.other.userId},{status:'RESOLVED'}])
      await h.denied(root,'POST',API,{title:'Forged create',...fields},400)
    await h.denied(root,'POST',API,{title:'x'.repeat(201)},400)
    await h.denied(root,'POST',API,{title:'Long description',description:'x'.repeat(8193)},400)
    let row = await h.create(root,'Concurrent version target',{priority:'HIGH'})
    await h.pace(); await h.pace()
    const simultaneous = await Promise.all([
      h.businessCall(root,'PATCH',ticketPath(row),{version:row.version,title:'Winner A'},{unpaced:true}),
      h.businessCall(root,'PATCH',ticketPath(row),{version:row.version,title:'Winner B'},{unpaced:true}),
    ])
    assert.deepEqual(simultaneous.map((response) => response.status).sort(),[200,409])
    row = simultaneous.find((response) => response.status === 200).data
    await h.denied(root,'DELETE',ticketPath(row),{version:1},409)
    for (const fields of [{tenantId:h.actors.remote.tenantId},{organizationId:h.actors.child.organizationId},{ownerUserId:h.actors.other.userId}])
      await h.denied(root,'PATCH',ticketPath(row),{version:row.version,...fields},400)
    await h.denied(root,'PATCH',ticketPath(row),{version:row.version,status:'RESOLVED'},400)
    for (const status of ['IN_PROGRESS','RESOLVED','IN_PROGRESS']) {
      row = await h.accepted(root,'PATCH',ticketPath(row),{version:row.version,status})
      assert.equal(row.status,status)
    }
    const filtered = await h.records(root,'&status=IN_PROGRESS&priority=HIGH')
    assert.deepEqual(sorted(filtered),[row.id])
    await h.denied(root,'GET',`${API}?status=INVALID`,undefined,400)
    await h.denied(root,'GET',`${API}?priority=INVALID`,undefined,400)
    await h.denied(root,'GET',`${API}?tenantId=${h.actors.remote.tenantId}&organizationId=${h.actors.remote.organizationId}`,undefined,400)
    const persisted = (await query(h.businessDb.writer,'SELECT tenant_id,organization_id,owner_user_id,version,status FROM work_orders WHERE id=$1',[row.id])).rows[0]
    assert.equal(persisted.tenant_id,root.tenantId); assert.equal(persisted.organization_id,root.organizationId); assert.equal(persisted.owner_user_id,root.userId); assert.equal(persisted.status,'IN_PROGRESS'); assert.equal(persisted.version,row.version)
  })
  await t.test('export permission and grant are independent and formulas are escaped', async () => {
    const formula = await h.create(root,'=SUM(1,2)',{description:'+cmd\n@SUM(A1)\tplain'})
    const csv = await h.businessCall(root,'GET',`${API}/export.csv`)
    assert.equal(csv.status,200); assert.ok(csv.text.includes(formula.number)); assert.ok(csv.text.includes("'=SUM(1,2)")); assert.ok(csv.text.includes("'+cmd"))
    await revokeData(h,root,'ticket','export')
    await h.denied(root,'GET',`${API}/export.csv`,undefined,403)
    assert.equal((await h.accepted(root,'GET',ticketPath(formula))).id,formula.id)
    await restoreData(h,root,'ticket','export')
    const menu = await revokeOperation(h,'ticket:export')
    await h.denied(root,'GET',`${API}/export.csv`,undefined,403)
    await restoreOperation(h,menu)
  })
  await t.test('each write action separately requires its direct membership data grant', async () => {
    for (const [action,method,endpoint,body] of [
      ['create','POST',API,{title:'No create grant'}],
      ['update','PATCH',ticketPath(tickets.ownRoot),{version:tickets.ownRoot.version,title:'No update grant'}],
      ['delete','DELETE',ticketPath(tickets.ownRoot),{version:tickets.ownRoot.version}],
    ]) {
      await revokeData(h,root,'ticket',action)
      await h.denied(root,method,endpoint,body,403)
      await restoreData(h,root,'ticket',action)
    }
  })
  await t.test('real reminder persists, reaches Socket and inbox, deduplicates and marks read', async () => {
    const recipient = await h.connectSocket(h.actors.other)
    const foreign = await h.connectSocket(h.actors.remote)
    const row = await h.create(root,'Reminder E2E',{assigneeUserId:h.actors.other.userId})
    const key = `work-order-reminder-${randomUUID()}`
    const first = await h.accepted(root,'POST',`${ticketPath(row)}/remind`,{idempotencyKey:key})
    assert.equal(first.duplicate,false); assert.equal(first.recipientCount,1); assert.equal(first.deliveryCount,1)
    const retry = await h.accepted(root,'POST',`${ticketPath(row)}/remind`,{idempotencyKey:key})
    assert.equal(retry.duplicate,true); assert.equal(retry.publicationId,first.publicationId)
    await until(() => recipient.events.length === 1,'real PostgreSQL notification must reach the authenticated Socket.IO recipient',12000)
    assert.equal(foreign.events.length,0)
    const stored = await query(h.platformDb.writer,'SELECT id,user_id,tenant_id,organization_id,link,is_read FROM notifications WHERE publication_id=$1',[first.publicationId])
    assert.equal(stored.rowCount,1)
    const notification = stored.rows[0]
    assert.equal(notification.user_id,h.actors.other.userId); assert.equal(notification.tenant_id,root.tenantId); assert.equal(notification.organization_id,root.organizationId)
    assert.ok(notification.link.startsWith(`/micro/${APP}`) && notification.link.includes(row.id))
    const inbox = await h.platformOk('GET','/api/notifications?size=200',h.actors.other.session)
    assert.ok(inbox.records.some((item) => item.id === notification.id))
    const foreignInbox = await h.platformOk('GET','/api/notifications?size=200',h.actors.remote.session)
    assert.ok(!foreignInbox.records.some((item) => item.id === notification.id))
    assert.deepEqual(await h.platformOk('PUT',`/api/notifications/${notification.id}/read`,h.actors.remote.session),{ok:false})
    assert.deepEqual(await h.platformOk('PUT',`/api/notifications/${notification.id}/read`,h.actors.other.session),{ok:true})
    assert.equal((await query(h.platformDb.writer,'SELECT is_read FROM notifications WHERE id=$1',[notification.id])).rows[0].is_read,true)
    assert.equal((await h.accepted(root,'GET',ticketPath(row))).version,row.version, 'reminder must not save or version the work order')
    await revokeData(h,root,'notification','publish')
    await h.denied(root,'POST',`${ticketPath(row)}/remind`,{idempotencyKey:`missing-grant-${randomUUID()}`},403)
    await restoreData(h,root,'notification','publish')
    const menu = await revokeOperation(h,'notification:publish')
    await h.denied(root,'POST',`${ticketPath(row)}/remind`,{idempotencyKey:`missing-operation-${randomUUID()}`},403)
    await restoreOperation(h,menu)
    recipient.socket.disconnect(); foreign.socket.disconnect()
  })
  await t.test('same-token permission/data/app/member revocation and real context switch fail closed', async () => {
    const actor = await h.login('p11-owner',root.organizationId)
    const token = await h.appToken(actor)
    await h.accepted(token,'GET',API)
    await revokeData(h,actor,'ticket','read')
    await h.denied(token,'GET',API,undefined,403)
    await restoreData(h,actor,'ticket','read')
    const menu = await revokeOperation(h,'ticket:update')
    await h.denied(token,'PATCH',ticketPath(tickets.ownRoot),{version:tickets.ownRoot.version,title:'Hidden button bypass'},403)
    await restoreOperation(h,menu)
    await query(h.platformDb.writer,'DELETE FROM tenant_applications WHERE tenant_id=$1 AND application_id=$2',[actor.tenantId,h.ids.application])
    await h.denied(token,'GET',API,undefined,401)
    await query(h.platformDb.writer,'INSERT INTO tenant_applications(tenant_id,application_id) VALUES($1,$2)',[actor.tenantId,h.ids.application])
    await query(h.platformDb.writer,'DELETE FROM organization_applications WHERE organization_id=$1 AND application_id=$2',[actor.organizationId,h.ids.application])
    await h.denied(token,'GET',API,undefined,401)
    await query(h.platformDb.writer,'INSERT INTO organization_applications(organization_id,application_id) VALUES($1,$2)',[actor.organizationId,h.ids.application])
    const switched = await h.platformOk('POST','/api/auth/context',actor.session,{organizationId:h.actors.child.organizationId})
    h.secrets.add(switched.accessToken)
    await h.denied(token,'GET',API,undefined,401)
    const next = { ...h.actors.ownChild,session:switched.accessToken,token:null }
    await h.setGrants(next,'ORG')
    assert.ok((await h.records(next)).every((row) => row.organizationId === next.organizationId))
    await h.platformOk('POST','/api/auth/logout',next.session)
    await h.denied(next.token,'GET',API,undefined,401)
    const disposable = await h.login('p11-peer',h.actors.peer.organizationId)
    const removedToken = await h.appToken(disposable)
    await query(h.platformDb.writer,'DELETE FROM memberships WHERE user_id=$1 AND organization_id=$2',[disposable.userId,disposable.organizationId])
    await h.denied(removedToken,'GET',API,undefined,401)
    // This membership is not used after removal; no test re-enables that SID.
    await h.setGrants(root,'TENANT')
    await h.setGrants(h.actors.ownChild,'TENANT')
  })
  await t.test('actual production host and Vue iframe CRUD, reminder, context switch and immutable release rollback', async () => {
    await checkedBrowserScenario(h,tickets)
  })
})

async function revokeData(h,actor,resource,action) {
  await query(h.platformDb.writer,`DELETE FROM membership_data_grants g USING memberships m
    WHERE m.id=g.membership_id AND m.user_id=$1 AND m.organization_id=$2 AND g.application_id=$3 AND g.resource_code=$4 AND g.action_code=$5`,[actor.userId,actor.organizationId,h.ids.application,resource,action])
}
async function restoreData(h,actor,resource,action) {
  await query(h.platformDb.writer,`INSERT INTO membership_data_grants(membership_id,application_id,resource_code,action_code,scope_type)
    SELECT id,$3,$4,$5,'TENANT' FROM memberships WHERE user_id=$1 AND organization_id=$2`,[actor.userId,actor.organizationId,h.ids.application,resource,action])
}
async function revokeOperation(h,permission) {
  const menu = (await query(h.platformDb.writer,'SELECT id FROM menus WHERE application_id=$1 AND permission=$2',[h.ids.application,permission])).rows[0].id
  await query(h.platformDb.writer,'DELETE FROM organization_menus WHERE organization_id=$1 AND menu_id=$2',[h.actors.root.organizationId,menu])
  return menu
}
async function restoreOperation(h,menu) {
  await query(h.platformDb.writer,'INSERT INTO organization_menus(organization_id,menu_id) VALUES($1,$2)',[h.actors.root.organizationId,menu])
}

async function browserScenario(h,tickets) {
  h.browserStage='register-preflight-activate-v1'
  const releases = h.debugReleases ?? {}
  for (const version of ['1.0.0','1.0.1'].filter((value) => !releases[value])) {
    const registered = await h.adminCommand(['register','--release-file',h.releaseFiles[version],'--apply'])
    assert.equal(registered.release.validationStatus,'DRAFT'); assert.equal(registered.releaseActivated,false)
    releases[version] = registered.release
    const preflight = await h.adminCommand(['preflight','--release-id',registered.release.id,'--apply'])
    assert.equal(preflight.release.validationStatus,'PASSED')
  }
  const activated = await h.adminCommand(['activate','--release-id',releases['1.0.0'].id,'--apply'])
  assert.equal(activated.deployment.version,'1.0.0')
  if(scenario==='browser') h.debugReleases=releases
  await h.setGrants(h.actors.root,'ORG')
  await h.setGrants(h.actors.ownChild,'ORG')
  // The preceding real HTTP revocation gate deliberately retires application
  // tokens. Restoring app authorization must not revive those tokens; the
  // independent concurrent writer explicitly obtains a newly issued token.
  h.actors.root.token=null
  const actor = await h.login('p11-owner',h.actors.root.organizationId)
  h.browserStage='launch-browser'
  const browser = await h.launchBrowser()
  const context = await browser.newContext({ acceptDownloads:true, viewport:{width:1440,height:1000} })
  const page = await context.newPage()
  page.setDefaultTimeout(20000)
  const errors = [], transportNotifications = [], businessRequests = [], asynchronous = [], resourceFailures=[]
  const hostAssetRequests = []
  let dropReminder = false, lostReminder, heldRead = null, holdRead = false, browserAppToken
  let resolveHeld
  const readHeld = new Promise((resolve) => { resolveHeld=resolve })
  let releaseRead
  const continueRead = new Promise((resolve) => { releaseRead=resolve })
  async function visual(name) {
    if (process.env.WORK_ORDERS_E2E_VISUAL_QA!=='1') return
    const file=h.qaImage(name)
    await page.locator('.micro-mount-zone').screenshot({path:file})
    console.log(`[Phase11 visual QA temporary image] ${imageDisplayPath(file)}`)
  }
  async function visibleEditorFooter(frame) {
    const bounds=await frame.getByTestId('ticket-editor').evaluate((modal) => {
      const box=modal.getBoundingClientRect(),footer=modal.querySelector('.modal-footer').getBoundingClientRect()
      return {height:innerHeight,top:box.top,bottom:box.bottom,footerTop:footer.top,footerBottom:footer.bottom}
    })
    assert.ok(bounds.top>=-1 && bounds.bottom<=bounds.height+1,'real editor must fit inside its iframe viewport')
    assert.ok(bounds.footerTop>=0 && bounds.footerBottom<=bounds.height+1,'save and cancel controls must remain inside the actual iframe viewport')
    console.log(`[Phase11 safe editor viewport] ${JSON.stringify(bounds)}`)
  }
  page.on('pageerror', (error) => {
    errors.push('production page runtime error');h.logs.push(error.message)
    let message=String(error.message)
    for(const secret of h.secrets) if(secret) message=message.replaceAll(secret,'[redacted]')
    if(/Authorization|Bearer|Basic|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/i.test(message)) message='Browser JavaScript error contained authentication text; details retained only in memory'
    console.log(`[Phase11 safe browser JS error] ${message.slice(0,240)}`)
    const sites=[]
    for(const line of String(error.stack ?? '').split('\n').slice(1)) {
      const match=line.match(/(https?:\/\/[^\s)]+\.m?js):(\d+):(\d+)/)
      if(!match) continue
      try { const url=new URL(match[1]);if(url.pathname.startsWith('/assets/') || url.pathname.startsWith(`/sub-apps/${APP}/versions/`)) sites.push({path:url.pathname,line:Number(match[2]),column:Number(match[3])}) } catch {}
    }
    console.log(`[Phase11 safe browser JS locations] ${JSON.stringify(sites.slice(0,4))}`)
  })
  page.on('console', (message) => h.logs.push(message.text()))
  page.on('request',(request) => {
    const url = new URL(request.url())
    if (url.pathname.startsWith('/assets/') && url.pathname.endsWith('.js')) hostAssetRequests.push(url.pathname)
  })
  page.on('websocket',(socket) => socket.on('framereceived',({payload}) => {
    if (typeof payload!=='string' || !payload.startsWith('42')) return
    try {
      const packet = JSON.parse(payload.slice(2))
      if (packet[0]==='notification') transportNotifications.push(packet[1])
    } catch {}
  }))
  page.on('response',(response) => {
    const pathname=new URL(response.url()).pathname
    if (response.status()>=400 && (pathname.startsWith('/assets/') || pathname.startsWith('/sub-apps/'))) resourceFailures.push({path:pathname,status:response.status()})
    if (!response.url().includes('/api/oauth/session-token') || response.status()!==201) return
    asynchronous.push((async () => {
      try { const value=await response.json(); if (typeof value.accessToken==='string') h.secrets.add(value.accessToken) } catch {}
    })())
  })
  await context.addInitScript(({token,app}) => {
    if(window.top!==window) return
    localStorage.setItem('dante-token',token)
    localStorage.setItem('dante-current-app',app)
    localStorage.setItem('dante-theme','light')
  },{token:actor.session,app:h.ids.application})
  // Scheduling and fault injection retain the actual request and actual API
  // response. One committed reminder reply is lost to test its real retry;
  // one actual list response is delayed across a real context switch.
  await context.route('**/work-orders-api/**',async (route) => {
    const request=route.request(),url=new URL(request.url())
    if (request.method()==='OPTIONS') { await route.continue(); return }
    const headers=await request.allHeaders()
    if (headers.authorization?.startsWith('Bearer ')) {
      browserAppToken=headers.authorization.slice(7); h.secrets.add(browserAppToken)
    }
    businessRequests.push({method:request.method(),path:url.pathname,origin:headers.origin})
    await h.pace()
    try {
      if (dropReminder && request.method()==='POST' && url.pathname.endsWith('/remind')) {
        dropReminder=false
        const response=await route.fetch({timeout:10000})
        assert.equal(response.status(),200,'transport-loss scenario must commit a real reminder first')
        lostReminder={body:request.postDataJSON(),result:await response.json()}
        await route.abort('failed')
        return
      }
      if (holdRead && request.method()==='GET' && url.pathname===API) {
        holdRead=false
        const response=await route.fetch({timeout:10000})
        assert.equal(response.status(),200)
        heldRead={response,data:await response.json(),token:headers.authorization.slice(7)}
        resolveHeld()
        await continueRead
        await route.fulfill({response})
        return
      }
      await route.continue()
    } catch {
      // A retired opaque iframe cancels its pending route. Its old response is
      // already captured; neither that transport nor its DTO may enter new UI.
      if (!heldRead || url.pathname!==API || request.method()!=='GET') errors.push('unexpected business browser route failure')
    }
  })
  try {
    h.browserStage='initial-production-iframe-mount'
    await page.goto(`${h.gatewayBase}/micro/${APP}`)
    const frame=page.frameLocator('.micro-mount-zone iframe')
    await frame.getByTestId('work-orders-app').waitFor()
    await frame.locator('[data-testid="app-version"][data-version="1.0.0"]').waitFor()
    await frame.locator(`[data-testid="work-orders-app"][data-org-id="${actor.organizationId}"]`).waitFor()
    await frame.locator(`[data-testid="ticket-row"][data-ticket-id="${tickets.ownRoot.id}"]`).waitFor()
    await visual('list-light')
    assert.equal(await page.locator('.micro-mount-zone iframe').count(),1)
    const sandbox=await page.locator('.micro-mount-zone iframe').getAttribute('sandbox')
    assert.deepEqual(sandbox.trim().split(/\s+/).sort(),['allow-forms','allow-scripts'],'actual iframe must retain the exact script/form-only opaque sandbox')
    const isolated=await frame.locator('body').evaluate(() => {
      let parentBlocked=false,storageBlocked=false
      try { parent.document.body } catch { parentBlocked=true }
      try { localStorage.getItem('dante-token') } catch { storageBlocked=true }
      return {parentBlocked,storageBlocked}
    })
    assert.deepEqual(isolated,{parentBlocked:true,storageBlocked:true})
    await page.evaluate(() => { window.__phase11HostRoot=document.querySelector('#root'); window.__phase11HostMarker='phase11-owned-host-document' })

    const title=`Browser real work order ${randomUUID().slice(0,8)}`
    h.browserStage='vue-create'
    const description='Plain text <img src=x onerror="window.__phase11Unsafe=true">\nSecond line'
    await frame.getByTestId('create-ticket').click()
    await frame.getByTestId('ticket-title').fill(title)
    await frame.getByTestId('ticket-description').fill(description)
    await frame.getByTestId('ticket-priority-input').selectOption('HIGH')
    await visibleEditorFooter(frame)
    await visual('editor-light')
    const createdResponse=page.waitForResponse((response) => new URL(response.url()).pathname===API && response.request().method()==='POST')
    await frame.getByTestId('save-ticket').click()
    const createdHttp=await createdResponse
    assert.equal(createdHttp.status(),201)
    let row=await createdHttp.json()
    assert.equal(row.tenantId,actor.tenantId); assert.equal(row.organizationId,actor.organizationId); assert.equal(row.ownerUserId,actor.userId)
    await frame.getByTestId('ticket-detail').getByRole('heading',{name:title,exact:true}).waitFor()
    assert.equal(await frame.locator('img').count(),0,'description must remain plain text in the actual Vue DOM')
    const visibleText=await frame.locator('body').innerText()
    assert.ok(!visibleText.includes(row.id) && !visibleText.includes(actor.tenantId) && !visibleText.includes(actor.userId))

    await frame.getByTestId('edit-ticket').click()
    h.browserStage='vue-conflict-edit'
    const editedTitle=`${title} edited in Vue`
    await frame.getByTestId('ticket-title').fill(editedTitle)
    const concurrent=await h.accepted(h.actors.root,'PATCH',`${API}/${row.id}`,{version:row.version,title:'Another real writer changed this work order'})
    const conflictResponse=page.waitForResponse((response) => new URL(response.url()).pathname===`${API}/${row.id}` && response.request().method()==='PATCH')
    await frame.getByTestId('save-ticket').click()
    assert.equal((await conflictResponse).status(),409)
    await frame.getByTestId('version-conflict').waitFor()
    assert.equal(await frame.getByTestId('ticket-title').inputValue(),editedTitle,'conflict must preserve the user draft')
    await frame.getByTestId('use-latest-version').waitFor()
    await until(async () => await frame.getByTestId('use-latest-version').isEnabled(),'actual latest work-order version must be available in the conflict dialog',20000)
    await visibleEditorFooter(frame)
    await visual('editor-conflict')
    await frame.getByTestId('use-latest-version').click()
    const updatedResponse=page.waitForResponse((response) => new URL(response.url()).pathname===`${API}/${row.id}` && response.request().method()==='PATCH')
    await frame.getByTestId('save-ticket').click()
    const updatedHttp=await updatedResponse
    assert.equal(updatedHttp.status(),200); row=await updatedHttp.json(); assert.equal(row.version,concurrent.version+1)
    await frame.getByTestId('ticket-detail').getByRole('heading',{name:editedTitle,exact:true}).waitFor()
    for (const status of ['IN_PROGRESS','RESOLVED','IN_PROGRESS']) {
      h.browserStage=`vue-status-${status}`
      const changedResponse=page.waitForResponse((response) => new URL(response.url()).pathname===`${API}/${row.id}` && response.request().method()==='PATCH')
      await frame.getByTestId('change-ticket-status').click()
      const changed=await changedResponse; assert.equal(changed.status(),200); row=await changed.json(); assert.equal(row.status,status)
      await frame.locator(`[data-testid="ticket-detail"] [data-version="${row.version}"]`).waitFor()
    }

    dropReminder=true
    h.browserStage='vue-reminder-lost-reply-retry-inbox-read'
    await page.getByLabel('实时推送已连接').waitFor()
    await frame.getByTestId('remind-ticket').click()
    await frame.getByRole('button',{name:'重试提醒',exact:true}).waitFor()
    await until(async () => !!lostReminder?.result.publicationId && await frame.getByRole('button',{name:'重试提醒',exact:true}).isEnabled(),
      'the first real reminder must commit and report unknown outcome before retry',20000)
    assert.ok(lostReminder?.result.publicationId)
    const reminderResponse=page.waitForResponse((response) => new URL(response.url()).pathname===`${API}/${row.id}/remind` && response.request().method()==='POST')
    await frame.getByTestId('remind-ticket').click()
    const reminderHttp=await reminderResponse
    assert.equal(reminderHttp.status(),200)
    const reminder=await reminderHttp.json()
    assert.equal(reminder.duplicate,true); assert.equal(reminder.publicationId,lostReminder.result.publicationId)
    assert.equal(reminderHttp.request().postDataJSON().idempotencyKey,lostReminder.body.idempotencyKey,'Vue retry must retain the same real business event key')
    const deliveries=await query(h.platformDb.writer,'SELECT id,title,is_read FROM notifications WHERE publication_id=$1',[reminder.publicationId])
    assert.equal(deliveries.rowCount,1)
    const notification=deliveries.rows[0]
    await until(() => transportNotifications.some((packet) => packet.id===notification.id),'actual host browser must receive the persisted notification over Socket.IO',12000)
    assert.equal(transportNotifications.filter((packet) => packet.id===notification.id).length,1)
    await page.getByRole('button',{name:/^(?:\d+ 条未读通知|通知)$/}).click()
    const notificationButton=page.getByRole('button').filter({has:page.getByText(notification.title,{exact:true})})
    await notificationButton.waitFor()
    const readResponse=page.waitForResponse((response) => new URL(response.url()).pathname===`/api/notifications/${notification.id}/read` && response.request().method()==='PUT')
    await notificationButton.click()
    assert.equal((await readResponse).status(),200)
    assert.equal((await query(h.platformDb.writer,'SELECT is_read FROM notifications WHERE id=$1',[notification.id])).rows[0].is_read,true)
    assert.equal((await h.accepted(h.actors.root,'GET',`${API}/${row.id}`)).version,row.version,'UI reminder and inbox read must not save the ticket')

    await frame.getByTestId('back-to-list').click()
    h.browserStage='vue-filter-csv-host-download'
    await frame.locator(`[data-testid="ticket-row"][data-ticket-id="${row.id}"]`).waitFor()
    await frame.getByTestId('filter-status').selectOption('IN_PROGRESS')
    await frame.getByTestId('filter-priority').selectOption('HIGH')
    await frame.locator(`[data-testid="ticket-row"][data-ticket-id="${row.id}"]`).waitFor()
    const downloadPromise=page.waitForEvent('download')
    await frame.getByTestId('export-tickets').click()
    const download=await downloadPromise
    assert.equal(download.suggestedFilename(),'工单.csv')
    assert.ok(download.url().startsWith(`blob:${h.gatewayBase}/`),'opaque iframe download must be brokered by the host document')
    const csvPath=await download.path()
    const downloaded=readFileSync(csvPath,'utf8')
    assert.ok(downloaded.includes(editedTitle) && !downloaded.includes(tickets.otherRoot.title),'actual brokered CSV must preserve current filters')
    assert.ok(downloaded.startsWith('\uFEFF'),'download must retain its UTF-8 CSV BOM')
    assert.ok(!sandbox.includes('allow-downloads'))
    await frame.locator(`[data-testid="ticket-row"][data-ticket-id="${row.id}"]`).getByTestId('open-ticket').click()
    h.browserStage='vue-soft-delete'
    await frame.getByTestId('delete-ticket').click()
    const deleteResponse=page.waitForResponse((response) => new URL(response.url()).pathname===`${API}/${row.id}` && response.request().method()==='DELETE')
    await frame.getByTestId('confirm-delete').click()
    assert.equal((await deleteResponse).status(),200)
    await frame.getByTestId('ticket-list').waitFor()
    await until(async () => !await frame.locator(`[data-testid="ticket-row"][data-ticket-id="${row.id}"]`).count(),'deleted work order must leave the real Vue list')
    assert.ok((await query(h.businessDb.writer,'SELECT deleted_at FROM work_orders WHERE id=$1',[row.id])).rows[0].deleted_at)

    await frame.getByTestId('filter-status').selectOption('ALL')
    await frame.getByTestId('filter-priority').selectOption('ALL')
    await frame.locator(`[data-testid="ticket-row"][data-ticket-id="${tickets.ownRoot.id}"]`).waitFor()
    if (process.env.WORK_ORDERS_E2E_VISUAL_QA==='1') {
      h.browserStage='visual-dark-narrow'
      await page.getByRole('button',{name:'切换到深色模式',exact:true}).click()
      await frame.locator('html[data-theme="dark"]').waitFor()
      await frame.locator(`[data-testid="ticket-row"][data-ticket-id="${tickets.ownRoot.id}"]`).waitFor()
      await visual('list-dark')
      await page.setViewportSize({width:390,height:844})
      await visual('list-dark-narrow')
      await page.setViewportSize({width:1440,height:1000})
      await page.getByRole('button',{name:'切换到浅色模式',exact:true}).click()
      await frame.locator('html[data-theme="light"]').waitFor()
      await frame.locator(`[data-testid="ticket-row"][data-ticket-id="${tickets.ownRoot.id}"]`).waitFor()
      console.log('[Phase11 visual QA] Owned screenshots retained until explicit review and cleanup')
      await h.visualCheckpoint()
    }
    holdRead=true
    h.browserStage='vue-context-switch-delayed-response'
    await frame.getByTestId('refresh-tickets').click()
    await Promise.race([readHeld,delay(20000).then(() => { throw new Error('actual old-context list response was not captured') })])
    assert.ok(heldRead.data.records.some((record) => record.id===tickets.ownRoot.id))
    const switchResponse=page.waitForResponse((response) => new URL(response.url()).pathname==='/api/auth/context' && response.request().method()==='POST')
    await page.getByRole('button',{name:'切换组织上下文',exact:true}).click()
    await page.getByRole('button').filter({has:page.getByText('Phase11 A child',{exact:true})}).click()
    const switchedHttp=await switchResponse
    assert.equal(switchedHttp.status(),200)
    const switched=await switchedHttp.json(); h.secrets.add(switched.accessToken)
    const child=h.actors.ownChild
    await frame.locator(`[data-testid="work-orders-app"][data-org-id="${child.organizationId}"]`).waitFor({timeout:30000})
    await frame.locator(`[data-testid="ticket-row"][data-ticket-id="${tickets.otherChild.id}"]`).waitFor()
    releaseRead()
    await delay(500)
    assert.equal(await frame.locator(`[data-testid="ticket-row"][data-ticket-id="${tickets.ownRoot.id}"]`).count(),0,'late response must not display previous organization rows')
    assert.ok(!(await frame.locator('body').innerText()).includes(tickets.otherRoot.title))
    await h.denied(heldRead.token,'GET',API,undefined,401)
    const childActor={session:switched.accessToken,userId:actor.userId,tenantId:actor.tenantId,organizationId:child.organizationId}
    assert.ok((await h.records(childActor)).every((record) => record.organizationId===child.organizationId))
    const sessionBeforeRelease=await page.evaluate(() => localStorage.getItem('dante-token'))
    h.secrets.add(sessionBeforeRelease)
    const hostRequestsBeforeRelease=hostAssetRequests.length
    await page.evaluate(() => { window.__phase11RetiredFrames=[] })
    for (const version of ['1.0.1','1.0.0']) {
      h.browserStage=`actual-release-${version}`
      await page.evaluate(() => window.__phase11RetiredFrames.push(document.querySelector('.micro-mount-zone iframe')))
      const deployed=version==='1.0.1'
        ? await h.adminCommand(['activate','--release-id',releases[version].id,'--apply'])
        : await h.adminCommand(['rollback','--apply'])
      assert.equal(deployed.deployment.version,version); assert.equal(deployed.deployment.activeReleaseId,releases[version].id)
      await frame.locator(`[data-testid="app-version"][data-version="${version}"]`).waitFor({timeout:55000})
      await frame.locator(`[data-testid="ticket-row"][data-ticket-id="${tickets.otherChild.id}"]`).waitFor()
      assert.equal(await page.locator('.micro-mount-zone iframe').count(),1)
      assert.ok(await page.evaluate(() => window.__phase11HostMarker==='phase11-owned-host-document' && window.__phase11HostRoot===document.querySelector('#root') && !window.__phase11RetiredFrames.at(-1).isConnected),'actual release replacement must retain the running production host document and retire the old iframe')
      assert.ok(await page.evaluate((expected) => localStorage.getItem('dante-token')===expected,sessionBeforeRelease),'release replacement must preserve the real platform session')
      assert.equal(hostAssetRequests.length,hostRequestsBeforeRelease,'independent release activation must not reload production host JavaScript')
      const current=await h.platformOk('GET',`/api/apps/${APP}/context`,await h.appToken(childActor))
      assert.equal(current.application.deploymentVersion,version); assert.equal(current.application.entryVersionId,releases[version].id)
      assert.ok((await h.records(childActor)).every((record) => record.organizationId===child.organizationId))
      const entry=await h.http(h.gatewayBase,'GET',h.manifests[version].entryUrl,null,undefined,{headers:{Origin:'null'}})
      assert.equal(entry.status,200); assert.equal(entry.headers.get('access-control-allow-origin'),'*'); assert.equal(entry.headers.get('cache-control'),'no-cache')
      const asset=await h.http(h.gatewayBase,'GET',`/sub-apps/${APP}/versions/${version}/assets/config.js`)
      assert.equal(asset.status,200); assert.match(asset.headers.get('content-type'),/javascript/); assert.match(asset.headers.get('cache-control'),/immutable/)
      if(scenario==='full') assert.deepEqual(inventory(h.hostRoot),h.hostHashes,'production host bytes must remain identical throughout activate and rollback')
    }
    const deployments=await h.adminCommand(['deployments'])
    assert.equal(deployments.deployments[0].activeReleaseId,releases['1.0.0'].id)
    assert.equal(deployments.deployments[0].previousReleaseId,releases['1.0.1'].id)
    assert.ok(businessRequests.filter((request) => ['GET','POST','PATCH','DELETE'].includes(request.method)).every((request) => request.origin==='null'),'all actual iframe business requests must use the opaque origin CORS contract')
    assert.ok(businessRequests.some((request) => request.method==='POST') && businessRequests.some((request) => request.method==='PATCH') && businessRequests.some((request) => request.method==='DELETE'))
    assert.equal(errors.length,0,'actual production host/Vue browser flow must have no runtime or unexpected transport errors')
  } catch(error) {
    h.logs.push(String(error?.message ?? 'browser failure'))
    const iframeFlags=[]
    for(const candidate of page.frames().filter((item) => item!==page.mainFrame())) {
      try { iframeFlags.push(await candidate.evaluate(() => ({vue:!!window.Vue,sdk:!!window.DanteMicroIframe?.connect,
        config:!!window.__WORK_ORDERS_CONFIG__,app:!!document.querySelector('[data-testid="work-orders-app"]'),scripts:document.scripts.length}))) } catch {}
    }
    console.log(`[Phase11 safe browser diagnostic] ${JSON.stringify({stage:h.browserStage,pageErrors:errors.length,resourceFailures,iframeFlags})}`)
    if (process.env.WORK_ORDERS_E2E_VISUAL_QA==='1') {
      try {
        const file=h.qaImage('browser-failure')
        await page.screenshot({path:file,fullPage:true})
        console.log(`[Phase11 visual QA temporary failure image] ${imageDisplayPath(file)}`)
        if(scenario==='browser' && process.env.WORK_ORDERS_E2E_DEBUG_HOLD==='1' && h.browserStage==='initial-production-iframe-mount') {
          console.log('[Phase11 DEBUG only] Owned fixture remains live for up to10min; create owned QA/resume marker after host fix to reload')
          if(await h.debugResume()) {
            await context.close()
            return await browserScenario(h,tickets)
          }
        } else await h.visualCheckpoint()
      } catch {}
    }
    throw error
  } finally {
    releaseRead()
    await Promise.allSettled(asynchronous)
    await context.close()
  }
}
function imageDisplayPath(value) {
  return /^\/mnt\/[a-z]\//.test(value) ? `${value[5].toUpperCase()}:\\${value.slice(7).replaceAll('/','\\')}` : value
}
async function checkedBrowserScenario(h,tickets) {
  try { await browserScenario(h,tickets) }
  catch(error) {
    h.logs.push(String(error?.message ?? 'browser failure'))
    if(error?.name==='AssertionError') {
      const safeValue=(value) => ['number','boolean'].includes(typeof value) ? value : typeof value
      const locations=String(error.stack ?? '').split('\n').map((line) => line.match(/platform-e2e\.mjs:(\d+):(\d+)/))
        .filter(Boolean).slice(0,2).map((match)=>({line:Number(match[1]),column:Number(match[2])}))
      console.log(`[Phase11 safe assertion diagnostic] ${JSON.stringify({operator:error.operator,actual:safeValue(error.actual),expected:safeValue(error.expected),locations})}`)
    }
    const category=typeof error?.category==='string' && /^[A-Z_]+$/.test(error.category) ? error.category
      : typeof error?.name==='string' && /^[A-Za-z]+$/.test(error.name) ? error.name : 'BROWSER_ERROR'
    throw new Error(`Actual production browser scenario failed at ${h.browserStage ?? 'unknown'} (${category})`)
  }
}
