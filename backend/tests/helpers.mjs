// Real HTTP fixture and independently owned PostgreSQL database. This is not the
// genuine platform E2E suite: only the platform responses below are test fixtures.
import assert from 'node:assert/strict'
import { createHash, randomUUID } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { createServer as createHttpServer } from 'node:http'
import { parseEnv } from 'node:util'
import pg from 'pg'

export const APP_CODE = 'work-orders'
export const CLIENT_ID = 'work-orders-test-client'
export const CLIENT_SECRET = 'work-orders-http-fixture-only-secret'
export const ACTIONS = ['read', 'create', 'update', 'delete', 'export']
export const API = '/work-orders-api/tickets'

export async function testDatabaseUrl() {
  const configured = process.env.WORK_ORDERS_TEST_DATABASE_URL ?? process.env.TEST_DATABASE_URL
  if (configured) return configured
  try {
    const values = parseEnv(await readFile(new URL('../../../../.env.test', import.meta.url), 'utf8'))
    return values.TEST_DATABASE_URL || undefined
  } catch (error) {
    if (error.code === 'ENOENT') return undefined
    throw new Error('Cannot parse ignored test database configuration')
  }
}

function safeDbError(stage, error) {
  const code = typeof error?.code === 'string' && /^[A-Z0-9_]+$/.test(error.code) ? error.code : 'DATABASE_ERROR'
  return new Error(`Work-order isolated database ${stage} failed (${code})`)
}

async function dbOperation(stage, operation) {
  try { return await operation() } catch (error) { throw safeDbError(stage, error) }
}

function isolatedUrl(source, name) {
  try {
    const url = new URL(source)
    if (!['postgres:', 'postgresql:'].includes(url.protocol)) throw new Error()
    url.pathname = `/${name}`
    for (const key of ['application_name', 'database', 'dbname']) url.searchParams.delete(key)
    return url.toString()
  } catch { throw new Error('Test database configuration must be a PostgreSQL URL') }
}

export async function isolatedDatabase(t, connectionString) {
  const name = `work_orders_test_${randomUUID().replaceAll('-', '')}`
  const expectedName = name
  let admin
  let pool
  let connected = false
  let owned = false
  try {
    admin = new pg.Client({ connectionString, application_name: 'work-orders-test-admin', connectionTimeoutMillis: 10_000, query_timeout: 10_000 })
    admin.on('error', () => {})
  } catch (error) { throw safeDbError('constructing administration connection', error) }
  // Register cleanup before connecting or migrating, including partial setup failures.
  t.after(async () => {
    const errors = []
    if (pool) {
      try { await pool.end() } catch { errors.push('business connection pool close failed') }
    }
    try {
      if (owned && connected) {
        assert.equal(name, expectedName)
        assert.match(name, /^work_orders_test_[0-9a-f]{32}$/)
        await dbOperation('removing owned database', () => admin.query(`DROP DATABASE "${name}" WITH (FORCE)`))
        owned = false
      }
    } catch { errors.push('owned database removal failed') }
    finally {
      try { await admin.end() } catch { errors.push('administration connection close failed') }
    }
    assert.deepEqual(errors, [], errors.join('; '))
  })
  await dbOperation('connecting administration', () => admin.connect())
  connected = true
  await dbOperation('creating owned database', () => admin.query(`CREATE DATABASE "${name}" TEMPLATE template0`))
  owned = true // Never drop a pre-existing database after a name collision.
  try {
    pool = new pg.Pool({ connectionString: isolatedUrl(connectionString, name), application_name: 'work-orders-test-business', max: 8,
      connectionTimeoutMillis: 10_000, query_timeout: 15_000, statement_timeout: 5000, idle_in_transaction_session_timeout: 15_000 })
    pool.on('error', () => {})
  } catch (error) { throw safeDbError('constructing isolated pool', error) }
  const actual = await dbOperation('checking owned database', () => pool.query('SELECT current_database() AS name'))
  assert.equal(actual.rows[0].name, name)
  const { migrate } = await import('../db.mjs')
  await dbOperation('running production migrations', () => migrate(pool))
  return { pool, query: (sql, values = []) => dbOperation('querying owned database', () => pool.query(sql, values)) }
}

export async function listen(server) {
  await new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => { server.removeListener('error', reject); resolve() })
  })
  return `http://127.0.0.1:${server.address().port}`
}

export async function closeHttp(server) {
  if (!server?.listening) return
  server.closeAllConnections()
  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()))
}

export function identity(userId = randomUUID(), tenantId = randomUUID(), organizationId = randomUUID()) {
  return { userId, tenantId, organizationId }
}

export function grantFor(actor, action, scope = 'TENANT', organizationIds = []) {
  return { resource: 'ticket', action, scope, tenantId: actor.tenantId, subjectUserId: actor.userId, organizationIds }
}

export function actorState(actor) {
  return {
    actor, active: true, permissions: ACTIONS.map((action) => `ticket:${action}`),
    // The legacy four-scope matrix explicitly models a trusted platform-root
    // ceiling. Organization-switch regressions override this with real IDs.
    organizationScopeIds: null,
    grants: ACTIONS.map((action) => grantFor(actor, action)),
    introPatch: {}, contextPatch: {}, memberPatch: {}, notificationPatch: {}, mode: {},
    calls: { introspect: 0, context: 0, assignee: 0, notification: 0 },
  }
}

export function replaceGrant(state, action, scope, organizationIds = []) {
  state.grants = state.grants.filter((grant) => grant.action !== action)
  if (scope) state.grants.push(grantFor(state.actor, action, scope, organizationIds))
}

function json(response, status, value) {
  response.writeHead(status, { 'content-type': 'application/json' })
  response.end(JSON.stringify(value))
}

export async function platformFixture(t) {
  const states = new Map()
  const members = new Map()
  const publications = new Map()
  const notifications = []
  const observations = []
  let trapCount = 0
  const server = createHttpServer(async (request, response) => {
    try {
      const url = new URL(request.url, 'http://fixture.invalid')
      if (url.pathname === '/redirect-trap') {
        trapCount += 1
        json(response, 200, {})
        return
      }
      let state
      let stage
      if (url.pathname === '/oauth/introspect') {
        const expected = `Basic ${Buffer.from(`${CLIENT_ID}:${CLIENT_SECRET}`).toString('base64')}`
        if (request.method !== 'POST' || request.headers.authorization !== expected) {
          json(response, 401, { active: false }); return
        }
        if (!request.headers['content-type']?.startsWith('application/x-www-form-urlencoded')) {
          json(response, 400, { error: 'fixture requires RFC 7662 form body' }); return
        }
        let body = ''
        for await (const part of request) body += part
        state = states.get(new URLSearchParams(body).get('token'))
        stage = 'introspect'
      } else {
        const token = request.headers.authorization?.startsWith('Bearer ') ? request.headers.authorization.slice(7) : ''
        state = states.get(token)
        if (url.pathname === `/api/apps/${APP_CODE}/context`) stage = 'context'
        else if (url.pathname === `/api/apps/${APP_CODE}/members` || url.pathname.startsWith(`/api/apps/${APP_CODE}/members/`)) stage = 'assignee'
        else if (url.pathname === `/api/apps/${APP_CODE}/notifications`) stage = 'notification'
        else { json(response, 404, { error: 'fixture route not found' }); return }
      }
      if (!state) { json(response, 401, { active: false }); return }
      state.calls[stage] += 1
      observations.push({ stage, method: request.method, path: url.pathname, actor: { ...state.actor } })
      const mode = state.mode[stage]
      if (mode === 'redirect') {
        response.writeHead(302, { location: '/redirect-trap' }); response.end(); return
      }
      if (mode === 'timeout') {
        await new Promise((resolve) => setTimeout(resolve, 900))
        if (response.destroyed) return
      }
      if (mode === 'network') { request.socket.destroy(); return }
      if (mode === 'invalid-json') { response.writeHead(200, { 'content-type': 'application/json' }); response.end('{ invalid'); return }
      if (mode === 'oversized') { json(response, 200, { padding: 'x'.repeat(2 * 1024 * 1024) }); return }
      if (Number.isInteger(mode)) { json(response, mode, { error: 'fixture denied upstream operation' }); return }
      if (stage === 'introspect') {
        json(response, 200, {
          active: state.active, aud: APP_CODE, sub: state.actor.userId,
          tenant_id: state.actor.tenantId, organization_id: state.actor.organizationId,
          token_type: 'Bearer', client_id: CLIENT_ID, scope: 'profile', jti: randomUUID(),
          iat: Math.floor(Date.now() / 1000), exp: Math.floor(Date.now() / 1000) + 300,
          ...state.introPatch,
        })
      } else if (stage === 'context') {
        json(response, 200, {
          contractVersion: '1.0', contextVersion: createHash('sha256').update(`fixture-${state.calls.context}`).digest('hex'), appCode: APP_CODE,
          expiresAt: new Date(Date.now() + 25_000).toISOString(),
          application: { id: randomUUID(), name: '工单测试', runtimeMode: 'IFRAME', entryType: 'HTML', entryUrl: '/fixture/index.html' },
          user: { id: state.actor.userId, username: 'fixture-user', displayName: 'Fixture User' },
          tenant: { id: state.actor.tenantId, code: 'fixture', name: 'Fixture Tenant' },
          organization: { id: state.actor.organizationId, name: 'Fixture Organization', path: 'Fixture' },
          roles: [], permissions: state.permissions, dataGrants: state.grants, menus: [],
          organizationScopeIds: state.organizationScopeIds,
          ...state.contextPatch,
        })
      } else if (stage === 'notification') {
        if (request.method !== 'POST' || !request.headers['content-type']?.startsWith('application/json')) {
          json(response, 400, { error: 'fixture requires notification POST JSON' }); return
        }
        let raw = ''
        for await (const part of request) raw += part
        const body = JSON.parse(raw)
        notifications.push(body)
        const namespace = JSON.stringify([state.actor.tenantId, state.actor.organizationId, APP_CODE, state.actor.userId, body.idempotencyKey])
        const hash = createHash('sha256').update(JSON.stringify(body)).digest('hex')
        const existing = publications.get(namespace)
        if (existing && existing.hash !== hash) { json(response, 409, { error: 'idempotency conflict' }); return }
        const result = existing?.result ?? { publicationId: randomUUID(), recipientCount: 1, deliveryCount: 1, duplicate: false, createdAt: new Date().toISOString() }
        if (!existing) publications.set(namespace, { hash, result })
        json(response, 200, { ...result, duplicate: Boolean(existing), ...state.notificationPatch })
      } else {
        if (request.method !== 'GET' || url.searchParams.get('resource') !== 'ticket' ||
            !['create', 'update'].includes(url.searchParams.get('action'))) {
          json(response, 400, { error: 'fixture requires bound ticket member query' }); return
        }
        const organizationId = url.searchParams.get('organizationId') || state.actor.organizationId
        const userId = url.pathname.startsWith(`/api/apps/${APP_CODE}/members/`)
          ? url.pathname.slice(`/api/apps/${APP_CODE}/members/`.length) : null
        const prefix = `${state.actor.tenantId}:${organizationId}:`
        if (userId) {
          const valid = members.has(`${prefix}${userId}`)
          json(response, valid ? 200 : 404, { id: userId, displayName: `Member ${userId.slice(0, 8)}`, organizationId, ...state.memberPatch })
        } else {
          const page = Number(url.searchParams.get('page') || 1)
          const size = Math.min(Number(url.searchParams.get('size') || 50), 50)
          const q = url.searchParams.get('q') || ''
          const records = [...members.keys()].filter((key) => key.startsWith(prefix)).map((key) => {
            const id = key.slice(prefix.length)
            return { id, displayName: `Member ${id.slice(0, 8)}`, organizationId }
          }).filter((record) => record.displayName.includes(q)).sort((a, b) => a.id.localeCompare(b.id))
          json(response, 200, { records: records.slice((page - 1) * size, page * size), total: records.length, page, size })
        }
      }
    } catch { if (!response.headersSent) json(response, 500, { error: 'HTTP fixture failure' }) }
  })
  const url = await listen(server)
  t.after(() => closeHttp(server))
  return {
    url, states, members, observations, notifications,
    get trapCount() { return trapCount },
    add(actor, label = randomUUID()) {
      const token = `fixture-app-${label}`
      const state = actorState(actor)
      states.set(token, state)
      members.set(`${actor.tenantId}:${actor.organizationId}:${actor.userId}`, true)
      return { token, state, actor }
    },
  }
}

export async function businessServer(t, pool, platformOrigin, overrides = {}) {
  const { createServer } = await import('../server.mjs')
  const config = {
    appCode: APP_CODE, clientId: CLIENT_ID, clientSecret: CLIENT_SECRET, platformOrigin,
    host: '127.0.0.1', port: 0, platformTimeoutMs: 250,
    maxPlatformBodyBytes: 256 * 1024, maxBodyBytes: 64 * 1024, allowedOrigins: ['null'],
    ...overrides,
  }
  const server = createServer({ config, pool })
  const url = await listen(server)
  t.after(() => closeHttp(server))
  return { url, server }
}

export async function call(base, token, method = 'GET', path = API, body, options = {}) {
  const headers = { ...options.headers }
  if (token) headers.authorization = `Bearer ${token}`
  if (body !== undefined) headers['content-type'] ??= 'application/json'
  const response = await fetch(`${base}${path}`, {
    method, headers, redirect: 'manual', signal: AbortSignal.timeout(options.timeoutMs ?? 10_000),
    body: body === undefined ? undefined : typeof body === 'string' ? body : JSON.stringify(body),
  })
  const text = await response.text()
  let value
  try { value = JSON.parse(text) } catch { value = null }
  return { status: response.status, value, text, headers: response.headers }
}

export async function accepted(base, token, method, path, body, expected = [200, 201]) {
  const result = await call(base, token, method, path, body)
  assert.ok(expected.includes(result.status), `${method} ${path} expected ${expected.join('/')} but got ${result.status} (${result.value?.code ?? 'no code'})`)
  return result.value
}

export async function denied(base, token, method, path, body, expected = [401, 403, 404]) {
  const result = await call(base, token, method, path, body)
  assert.ok(expected.includes(result.status), `${method} ${path} expected rejection ${expected.join('/')} but got ${result.status}`)
  return result
}

export async function createTicket(base, actor, title, extras = {}) {
  return accepted(base, actor.token, 'POST', API, { title, ...extras })
}

export async function records(base, actor, query = '') {
  const page = await accepted(base, actor.token, 'GET', `${API}?page=1&size=100${query}`)
  assert.ok(Array.isArray(page.records), 'list must use the paging records contract')
  assert.equal(typeof page.total, 'number')
  return page.records
}

export async function matrix(t, connectionString) {
  const database = await isolatedDatabase(t, connectionString)
  const platform = await platformFixture(t)
  const service = await businessServer(t, database.pool, platform.url)
  const tenantA = randomUUID()
  const tenantB = randomUUID()
  const root = randomUUID()
  const child = randomUUID()
  const peer = randomUUID()
  const remote = randomUUID()
  const user = randomUUID()
  const actors = {
    root: platform.add(identity(user, tenantA, root), 'a-root'),
    other: platform.add(identity(randomUUID(), tenantA, root), 'a-root-other'),
    ownChild: platform.add(identity(user, tenantA, child), 'a-child-own'),
    child: platform.add(identity(randomUUID(), tenantA, child), 'a-child-other'),
    peer: platform.add(identity(randomUUID(), tenantA, peer), 'a-peer'),
    ownRemote: platform.add(identity(user, tenantB, remote), 'b-same-global-user'),
    remote: platform.add(identity(randomUUID(), tenantB, remote), 'b-other'),
  }
  const tickets = {
    ownRoot: await createTicket(service.url, actors.root, 'Own root ticket'),
    otherRoot: await createTicket(service.url, actors.other, 'Other root ticket'),
    ownChild: await createTicket(service.url, actors.ownChild, 'Own child ticket'),
    otherChild: await createTicket(service.url, actors.child, 'Other child ticket'),
    peer: await createTicket(service.url, actors.peer, 'Peer ticket'),
    ownRemote: await createTicket(service.url, actors.ownRemote, 'Same user another tenant'),
    remote: await createTicket(service.url, actors.remote, 'Other tenant ticket'),
  }
  for (const ticket of Object.values(tickets)) {
    assert.match(ticket.id, /^[0-9a-f-]{36}$/i)
    assert.equal(ticket.status, 'OPEN')
    assert.equal(ticket.priority, 'NORMAL')
    assert.equal(ticket.version, 1)
  }
  return { ...database, platform, ...service, actors, tickets, tenantA, tenantB, root, child, peer, remote }
}
