// A real HTTP preflight/trace check; no platform, business database or authentication bypass.
import assert from 'node:assert/strict'
import test from 'node:test'
import { randomUUID } from 'node:crypto'
import { createServer } from '../server.mjs'

test('opaque SDK request identifiers pass exact-origin CORS without enabling credentials or bypassing authentication', async t => {
  let databaseCalls = 0, platformCalls = 0
  const server = createServer({ config: { allowedOrigins: ['null', 'https://approved.example.test'] },
    pool: { query() { databaseCalls++; throw new Error('Unexpected database access') } },
    fetchImpl() { platformCalls++; throw new Error('Unexpected platform access') },
    logger: { info() {}, error() {} } })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)) })
  const url = `http://127.0.0.1:${server.address().port}/work-orders-api/tickets`
  const preflight = (origin, headers = 'authorization,content-type,x-request-id', method = 'PATCH') => fetch(url, {
    method: 'OPTIONS', headers: { Origin: origin, 'Access-Control-Request-Method': method, 'Access-Control-Request-Headers': headers },
  })
  for (const origin of ['null', 'https://approved.example.test']) {
    const response = await preflight(origin)
    assert.equal(response.status, 204)
    assert.equal(response.headers.get('access-control-allow-origin'), origin)
    assert.match(response.headers.get('access-control-allow-headers'), /x-request-id/i)
    assert.match(response.headers.get('access-control-expose-headers'), /x-request-id/i)
    assert.equal(response.headers.get('access-control-allow-credentials'), null)
  }
  const trace = randomUUID()
  const unauthorized = await fetch(url, { headers: { Origin: 'null', 'X-Request-Id': trace } })
  assert.equal(unauthorized.status, 401)
  assert.equal(unauthorized.headers.get('x-request-id'), trace)
  assert.match(unauthorized.headers.get('access-control-expose-headers'), /x-request-id/i)
  assert.equal(unauthorized.headers.get('access-control-allow-credentials'), null)
  assert.equal((await preflight('https://unapproved.example.test')).status, 403)
  for (const headers of ['authorization,cookie', 'authorization,x-unapproved-header']) {
    assert.equal((await preflight('null', headers)).status, 403)
  }
  assert.equal((await preflight('null', 'x-request-id', 'PUT')).status, 403)
  assert.equal(databaseCalls, 0); assert.equal(platformCalls, 0)
})
