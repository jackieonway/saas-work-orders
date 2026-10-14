import assert from 'node:assert/strict'
import test from 'node:test'
import { WorkOrdersClient, BusinessError } from '../src/client.mjs'
import { canTicket, canAssign, canRemind, visibleScope, validateDraft, routeView, statusChoices, MAX_BYTES } from '../src/domain.mjs'

const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done }); return { promise, resolve } }
function props(extra = {}) {
  return { contractVersion: '2.0', appCode: 'work-orders', basePath: '/micro/work-orders', contextVersion: 'ctx-1', contextEpoch: 1,
    token: 'mock-app-token', expiresAt: new Date(Date.now() + 60_000).toISOString(), tokenExpiresAt: new Date(Date.now() + 60_000).toISOString(),
    signal: new AbortController().signal, user: { id: 'user-1' }, tenant: { id: 'tenant-1' }, organization: { id: 'org-1' },
    organizationScopeIds: ['org-1', 'org-2'],
    permissions: ['ticket:read', 'ticket:create', 'ticket:update', 'ticket:delete', 'ticket:export', 'notification:publish'],
    dataGrants: ['read', 'create', 'update', 'delete', 'export'].map((action) => grant('SELF', action)), ...extra }
}
function grant(scope = 'SELF', action = 'read', extra = {}) {
  const organizationIds = scope === 'TENANT' ? [] : scope === 'ORG' ? ['org-1'] : ['org-1', 'org-2']
  return { resource: 'ticket', action, scope, subjectUserId: 'user-1', tenantId: 'tenant-1', organizationIds, ...extra }
}
const ticket = (extra = {}) => ({ id: 'ticket-1', tenantId: 'tenant-1', organizationId: 'org-1', ownerUserId: 'user-1', assigneeUserId: 'user-2', ...extra })

test('SELF follows creator ownership and never assignee identity; missing functional/data authorization stays hidden', () => {
  assert.equal(canTicket(props(), 'read', ticket()), true)
  assert.equal(canTicket(props(), 'read', ticket({ ownerUserId: 'other', assigneeUserId: 'user-1' })), false)
  assert.equal(canTicket(props({ permissions: [] }), 'read', ticket()), false)
  assert.equal(canTicket(props({ dataGrants: [] }), 'read', ticket()), false)
  assert.equal(canTicket(props(), 'read', ticket({ tenantId: 'other-tenant' })), false)
  assert.equal(canTicket(props({ dataGrants: [grant('constructor')] }), 'create'), false)
})

test('ORG/ORG_SUBTREE/TENANT scopes and cross-organization assignment follow distinct bounds', () => {
  const other = ticket({ organizationId: 'org-2', ownerUserId: 'other' })
  assert.equal(canTicket(props({ dataGrants: [grant('ORG')] }), 'read', other), false)
  assert.equal(canTicket(props({ dataGrants: [grant('ORG_SUBTREE')] }), 'read', other), true)
  assert.equal(canTicket(props({ dataGrants: [grant('TENANT')] }), 'read', other), true)
  const originalOwn = ticket({ organizationId: 'org-2' })
  assert.equal(canTicket(props(), 'update', originalOwn), true)
  assert.equal(canAssign(props(), originalOwn), false)
  assert.equal(canAssign(props({ dataGrants: [grant('ORG_SUBTREE', 'update')] }), originalOwn), true)
  assert.equal(visibleScope(props()), 'SELF')
})

test('child-context presentation denies owned ancestor and sibling tickets across every grant scope', () => {
  for (const scope of ['SELF', 'ORG_SUBTREE', 'TENANT']) {
    const dataGrants = ['read', 'update', 'delete'].map((action) => grant(scope, action,
      { organizationIds: scope === 'TENANT' ? [] : ['org-1', 'org-2', 'org-parent', 'org-sibling'] }))
    dataGrants.push(grant(scope, 'publish', { resource: 'notification',
      organizationIds: scope === 'TENANT' ? [] : ['org-1', 'org-2', 'org-parent', 'org-sibling'] }))
    const context = props({ dataGrants })
    assert.equal(canTicket(context, 'read', ticket({ organizationId: 'org-2' })), true)
    for (const organizationId of ['org-parent', 'org-sibling']) {
      const row = ticket({ organizationId, assigneeUserId: 'user-1' })
      for (const action of ['read', 'update', 'delete']) assert.equal(canTicket(context, action, row), false)
      assert.equal(canAssign(context, row), false)
      assert.equal(canRemind(context, row), false)
    }
    if (scope === 'TENANT') assert.equal(visibleScope(context), 'ORG_SUBTREE')
  }
})

test('legacy presentation context defaults to current organization and invalid ceilings expose no actions', () => {
  const legacy = props({ organizationScopeIds: undefined, dataGrants: [grant('TENANT')] })
  assert.equal(canTicket(legacy, 'read', ticket()), true)
  assert.equal(canTicket(legacy, 'read', ticket({ organizationId: 'org-2' })), false)
  assert.equal(visibleScope(legacy), 'ORG')
  for (const organizationScopeIds of [[], ['org-sibling'], ['org-1', null], 'org-1']) {
    assert.equal(canTicket(props({ organizationScopeIds }), 'create'), false)
  }
  const platformRoot = props({ organizationScopeIds: null, dataGrants: [grant('TENANT')] })
  assert.equal(canTicket(platformRoot, 'read', ticket({ organizationId: 'org-sibling' })), true)
  assert.equal(visibleScope(platformRoot), 'TENANT')
})

test('manual reminder requires ticket read plus independent notification permission/grant for its receiver', () => {
  assert.equal(canRemind(props(), ticket()), false)
  const allowed = props({ dataGrants: [grant(), grant('ORG', 'publish', { resource: 'notification' })] })
  assert.equal(canRemind(allowed, ticket()), true)
  assert.equal(canRemind({ ...allowed, permissions: ['notification:publish'] }, ticket()), false)
  assert.equal(canRemind({ ...allowed, dataGrants: [grant(), grant('SELF', 'publish', { resource: 'notification' })] }, ticket()), false)
})

test('drafts validate Unicode length and route parsing keeps resource ids out of business inputs', () => {
  assert.equal(validateDraft({ title: '工单', description: '', priority: 'NORMAL', status: 'OPEN' }), null)
  assert.match(validateDraft({ title: ' '.repeat(3), description: '', priority: 'NORMAL', status: 'OPEN' }), /标题/)
  assert.match(validateDraft({ title: '😀'.repeat(201), description: '', priority: 'NORMAL', status: 'OPEN' }), /200/)
  assert.deepEqual(routeView('/micro/work-orders/tickets/record-1'), { kind: 'detail', id: 'record-1' })
  assert.deepEqual(routeView('/micro/work-orders/tickets/new'), { kind: 'create' })
})

test('business HTTP uses captured application token, omits cookies and calls fetch with its native global receiver', async () => {
  const requests = [], client = new WorkOrdersClient({ fetchImpl: async function (url, options) {
    assert.equal(this, undefined, 'Window.fetch cannot be called with a WorkOrdersClient receiver')
    requests.push({ url, ...options }); return new Response(JSON.stringify({ records: [], total: 0 }), { headers: { 'Content-Type': 'application/json' } })
  } })
  client.update(props()); await client.list({ status: 'OPEN', priority: 'HIGH' })
  assert.equal(requests[0].headers.get('Authorization'), 'Bearer mock-app-token')
  assert.equal(requests[0].credentials, 'omit')
  assert.equal(new URL(requests[0].url, 'https://app.test').searchParams.get('status'), 'OPEN')
  assert.equal(requests[0].url.includes('mock-app-token'), false)
  client.stop()
})

test('editor status choices permit only staying, beginning, resolving and reopening', () => {
  assert.deepEqual(Object.keys(statusChoices('OPEN')), ['OPEN', 'IN_PROGRESS'])
  assert.deepEqual(Object.keys(statusChoices('IN_PROGRESS')), ['IN_PROGRESS', 'RESOLVED'])
  assert.deepEqual(Object.keys(statusChoices('RESOLVED')), ['RESOLVED', 'IN_PROGRESS'])
  assert.deepEqual(statusChoices('constructor'), {})
})

test('assignee directory can search and page beyond the first 50 direct members', async () => {
  let url
  const client = new WorkOrdersClient({ fetchImpl: async (requested) => {
    url = requested; return new Response(JSON.stringify({ records: [], total: 51, page: 2, size: 50 }))
  } })
  client.update(props()); await client.assignees('update', 'org-1', undefined, { q: '王', page: 2, size: 50 })
  const params = new URL(url, 'https://app.test').searchParams
  assert.equal(params.get('q'), '王'); assert.equal(params.get('page'), '2'); assert.equal(params.get('action'), 'update')
  client.stop()
})

test('context replacement immediately aborts requests and rejects delayed bodies despite a noncooperative transport', async () => {
  const body = deferred(), controller = new AbortController()
  let requestSignal, cleared = 0
  const client = new WorkOrdersClient({ onInvalidate: () => { cleared++ }, fetchImpl: async (_, options) => {
    requestSignal = options.signal
    return { ok: true, status: 200, headers: new Headers(), text: () => body.promise }
  } })
  client.update(props({ signal: controller.signal }))
  const requesting = client.detail('ticket-1')
  await Promise.resolve(); controller.abort()
  assert.equal(requestSignal.aborted, true)
  assert.equal(client.current(), null)
  client.update(props({ token: 'new-app-token', contextEpoch: 2, organization: { id: 'org-2' } }))
  body.resolve(JSON.stringify({ oldOrganization: true }))
  await assert.rejects(requesting, (error) => error.name === 'AbortError')
  assert.ok(cleared >= 2)
  client.stop()
})

test('version conflicts preserve HTTP status for explicit review instead of automatically overwriting a record', async () => {
  const client = new WorkOrdersClient({ fetchImpl: async () => new Response(JSON.stringify({ code: 'CONFLICT', message: '工单已更新' }), { status: 409 }) })
  client.update(props())
  await assert.rejects(client.updateTicket('ticket-1', { version: 1, title: 'draft' }), (error) => error instanceof BusinessError && error.status === 409)
  client.stop()
})

test('an authoritative 401 invalidates current context and rejects another delayed response', async () => {
  const body = deferred()
  let pendingSignal
  const client = new WorkOrdersClient({ fetchImpl: async (url, options) => {
    if (url.includes('/tickets/ticket-1')) {
      pendingSignal = options.signal
      return { ok: true, status: 200, headers: new Headers(), text: () => body.promise }
    }
    return new Response(JSON.stringify({ message: '已撤销' }), { status: 401 })
  } })
  client.update(props())
  const pending = client.detail('ticket-1')
  await Promise.resolve()
  await assert.rejects(client.list({}), (error) => error.status === 401)
  assert.equal(client.current(), null); assert.equal(pendingSignal.aborted, true)
  body.resolve(JSON.stringify({ title: '旧响应不可见' }))
  await assert.rejects(pending, (error) => error.name === 'AbortError')
})

test('CSV text preserves UTF-8 BOM exactly and rejects data beyond broker capacity', async () => {
  const csv = '\ufeff工单编号,标题\r\nWO-01,\'=1+1\r\n'
  const client = new WorkOrdersClient({ fetchImpl: async () => new Response(new TextEncoder().encode(csv), { headers: { 'Content-Type': 'text/csv' } }) })
  client.update(props()); assert.equal(await client.exportCsv({}), csv); client.stop()
  const oversized = new WorkOrdersClient({ fetchImpl: async () => new Response('x', { headers: { 'Content-Length': String(MAX_BYTES + 1) } }) })
  oversized.update(props()); await assert.rejects(oversized.exportCsv({}), (error) => error.status === 413); oversized.stop()
})

test('independent manual reminder sends only stable intent key, never another ticket save', async () => {
  const requests = [], client = new WorkOrdersClient({ fetchImpl: async (url, options) => {
    requests.push({ url, body: JSON.parse(options.body), method: options.method }); return new Response(JSON.stringify({ duplicate: false }))
  } })
  client.update(props()); await client.remind('ticket-1', 'ticket:ticket-1:manual:event')
  assert.deepEqual(requests, [{ url: '/work-orders-api/tickets/ticket-1/remind', method: 'POST', body: { idempotencyKey: 'ticket:ticket-1:manual:event' } }])
  client.stop()
})
