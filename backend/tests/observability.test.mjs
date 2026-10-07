// Actual loopback HTTP, with deliberately explicit schema/platform fixtures.
import assert from 'node:assert/strict'
import test from 'node:test'
import http from 'node:http'
import { randomUUID } from 'node:crypto'
import { createServer } from '../server.mjs'
import { loadConfig } from '../config.mjs'

const TOKEN = 'FixtureOnlyMetricsToken0123456789AbCd'
async function fixture(t) {
  const ids = { user: randomUUID(), tenant: randomUUID(), organization: randomUUID(), app: randomUUID() }
  const state = { database: true, platform: true, calls: [], logs: [] }
  const platform = http.createServer((req, res) => {
    state.calls.push({ path: req.url, requestId: req.headers['x-request-id'] })
    res.setHeader('content-type', 'application/json')
    if (!state.platform) { res.writeHead(503); res.end('{}'); return }
    if (!req.headers.authorization) { res.writeHead(401); res.end('{}'); return }
    if (req.url === '/oauth/introspect') res.end(JSON.stringify({ active: true, aud: 'work-orders', sub: ids.user,
      tenant_id: ids.tenant, organization_id: ids.organization, exp: Math.floor(Date.now() / 1000) + 120, token_type: 'Bearer' }))
    else res.end(JSON.stringify({ contractVersion: '1.0', appCode: 'work-orders', contextVersion: 'a'.repeat(64),
      expiresAt: new Date(Date.now() + 20000).toISOString(), user: { id: ids.user, username: 'fixture' },
      tenant: { id: ids.tenant }, organization: { id: ids.organization, name: 'Fixture organization' }, application: { id: ids.app },
      permissions: ['ticket:read'], dataGrants: [{ resource: 'ticket', action: 'read', scope: 'SELF',
        subjectUserId: ids.user, tenantId: ids.tenant, organizationIds: [] }] }))
  })
  await new Promise((resolve) => platform.listen(0, '127.0.0.1', resolve))
  const config = loadConfig({ WORK_ORDERS_DATABASE_URL: 'postgresql://fixture@127.0.0.1/no_connection',
    PLATFORM_API_ORIGIN: `http://127.0.0.1:${platform.address().port}`, PLATFORM_CLIENT_ID: 'fixture',
    PLATFORM_CLIENT_SECRET: 'fixture-never-logged', METRICS_TOKEN: TOKEN })
  const pool = { async query(sql) {
    if (!state.database) throw new Error('fixture-database-secret')
    return { rows: /count\(\*\)/.test(sql) ? [{ total: 0 }] : [], rowCount: 0 }
  } }
  const log = (value) => state.logs.push(value)
  const server = createServer({ config, pool, logger: { info: log, error: log } })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  t.after(async () => { for (const item of [server, platform]) { item.closeAllConnections(); await new Promise((resolve) => item.close(resolve)) } })
  return { state, ids, url: `http://127.0.0.1:${server.address().port}/work-orders-api` }
}

test('HTTP fixtures: metrics deny absent/wrong auth, UUID trace propagates, and metrics/logs do not reveal user data', async (t) => {
  const { state, ids, url } = await fixture(t)
  for (const authorization of [undefined, 'Bearer wrong', 'Basic ' + TOKEN]) {
    assert.equal((await fetch(url + '/metrics', { headers: authorization ? { authorization } : {} })).status, 401)
  }
  const trace = randomUUID(), secret = 'fixture-app-token-never-log'
  const response = await fetch(url + '/tickets?status=OPEN', { headers: { authorization: 'Bearer ' + secret, 'x-request-id': trace } })
  assert.equal(response.status, 200); assert.equal(response.headers.get('x-request-id'), trace)
  assert.ok(state.calls.filter((call) => call.path === '/oauth/introspect' || call.path.endsWith('/context')).every((call) => call.requestId === trace))
  const failed = await fetch(url + '/tickets?title=private-title', { headers: { 'x-request-id': 'unsafe-injected-value' } })
  assert.equal(failed.status, 401); assert.match(failed.headers.get('x-request-id'), /^[0-9a-f-]{36}$/)
  const metrics = await fetch(url + '/metrics', { headers: { authorization: 'Bearer ' + TOKEN } })
  assert.equal(metrics.status, 200)
  const body = await metrics.text()
  assert.match(body, /saas_http_auth_denials_total/); assert.match(body, /saas_http_request_duration_seconds_bucket/)
  for (const value of [secret, TOKEN, trace, ...Object.values(ids), 'private-title', '?status=OPEN']) assert.ok(!body.includes(value))
  const logs = state.logs.map(JSON.parse)
  const logged = logs.find((entry) => entry.request_id === trace)
  assert.equal(logged.tenant_id, ids.tenant); assert.equal(logged.organization_id, ids.organization)
  for (const value of [secret, TOKEN, ids.user, 'private-title', '?status=OPEN', 'unsafe-injected-value']) assert.ok(!state.logs.join('\n').includes(value))
})

test('HTTP fixtures: readiness tracks required schema and platform independently while liveness stays UP', async (t) => {
  const { state, url } = await fixture(t)
  assert.equal((await fetch(url + '/health')).status, 200)
  state.database = false
  let response = await fetch(url + '/health'); assert.equal(response.status, 503); assert.equal((await response.json()).checks.database, 'DOWN')
  state.database = true; state.platform = false
  response = await fetch(url + '/health'); assert.equal(response.status, 503); assert.equal((await response.json()).checks.platform, 'DOWN')
  assert.equal((await fetch(url + '/health/live')).status, 200)
  state.platform = true; assert.equal((await fetch(url + '/health')).status, 200)
  assert.ok(!state.logs.join('\n').includes('fixture-database-secret'))
})
