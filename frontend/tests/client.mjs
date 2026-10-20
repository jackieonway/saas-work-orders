import assert from 'node:assert/strict'
import test from 'node:test'
import { WorkOrdersClient, BusinessError } from '../src/client.mjs'
import { IntentStore } from '../src/intents.mjs'
import { canTicket, canAssign, canRemind, visibleScope, validateDraft, routeView, statusChoices, MAX_BYTES } from '../src/domain.mjs'

function delayedResponse(promise, status = 200) { return new Response(new ReadableStream({ async pull(controller) { const value = await promise; try { controller.enqueue(new TextEncoder().encode(value)); controller.close() } catch {} } }), { status }) }
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
const serverTicket = (extra = {}) => ({ id: '11111111-1111-4111-8111-111111111111', number: 'WO-000001',
  tenantId: '22222222-2222-4222-8222-222222222222', organizationId: '33333333-3333-4333-8333-333333333333',
  ownerUserId: '44444444-4444-4444-8444-444444444444', assigneeUserId: null, assigneeDisplayName: null,
  organizationName: 'Organization', ownerDisplayName: 'Owner', title: 'Original', description: '', priority: 'NORMAL', status: 'OPEN',
  version: 1, createdAt: '2026-10-17T00:00:00.000Z', updatedAt: '2026-10-17T00:00:00.000Z', ...extra })
const publication = (extra = {}) => ({ publicationId: '55555555-5555-4555-8555-555555555555', recipientCount: 1,
  deliveryCount: 1, duplicate: false, createdAt: '2026-10-17T00:00:00Z', ...extra })

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
  assert.deepEqual(routeView('/micro/work-orders/ticket-create'), { kind: 'create' })
})

test('creation has a separate route and the legacy new identifier cannot open a creation editor', () => {
  assert.deepEqual(routeView('/micro/work-orders/ticket-create/'), { kind: 'create' })
  assert.deepEqual(routeView('/micro/work-orders/tickets/new'), { kind: 'detail', id: 'new' })
  assert.deepEqual(routeView('/micro/work-orders/tickets/ticket-create'), { kind: 'detail', id: 'ticket-create' })
  assert.deepEqual(routeView('/micro/work-orders-other/ticket-create'), { kind: 'list' })
})

test('business HTTP uses captured application token, omits cookies and calls fetch with its native global receiver', async () => {
  const requests = [], client = new WorkOrdersClient({ fetchImpl: async function (url, options) {
    assert.equal(this, undefined, 'Window.fetch cannot be called with a WorkOrdersClient receiver')
    requests.push({ url, ...options }); return new Response(JSON.stringify({ records: [], total: 0, page: 1, size: 20 }), { headers: { 'Content-Type': 'application/json' } })
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
    return delayedResponse(body.promise)
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

test('theme and route updates retain pending business requests in the same authorization lifetime', async () => {
  const waiting = deferred(), host = new AbortController(), initial = props({ theme: 'light', routePath: '/micro/work-orders', signal: host.signal })
  let requestSignal, invalidations = 0
  const client = new WorkOrdersClient({ onInvalidate: () => { invalidations++ }, fetchImpl: async (_, options) => {
    requestSignal = options.signal; return delayedResponse(waiting.promise, 201)
  } })
  client.update(initial)
  const snapshot = client.capture(), pending = client.create({ title: 'draft' }, 'create:theme-update')
  const outcome = pending.then(value => ({ value }), error => ({ error }))
  await Promise.resolve()
  client.update({ ...initial, theme: 'dark' })
  client.update({ ...initial, theme: 'dark', routePath: '/micro/work-orders/ticket-create' })
  assert.equal(requestSignal.aborted, false)
  assert.equal(client.isCurrent(snapshot), true)
  assert.equal(invalidations, 1)
  waiting.resolve(JSON.stringify(serverTicket()))
  assert.deepEqual(await outcome, { value: serverTicket() })
  host.abort()
  assert.equal(client.current(), null)
  assert.equal(invalidations, 2, 'the reused lifetime still invalidates after the host aborts its original signal')
  client.stop()
})

test('same epoch and signal cannot hide changed authorization or an invalid replacement context', async () => {
  for (const change of [{ token: 'replacement' }, { expiresAt: new Date(Date.now() + 120000).toISOString() },
    { tokenExpiresAt: new Date(Date.now() + 120000).toISOString() }, { dataGrants: [] }, { permissions: [] },
    { deploymentVersion: 'v2', entryVersionId: 'new-release' }, { futureAuthorizationField: true }]) {
    const client = new WorkOrdersClient(), initial = props()
    client.update(initial)
    const old = client.capture()
    client.update({ ...initial, ...change })
    assert.equal(client.isCurrent(old), false, Object.keys(change).join(','))
    client.stop()
  }
  const client = new WorkOrdersClient(), initial = props()
  client.update(initial)
  assert.throws(() => client.update({ ...initial, contractVersion: '99.0' }), error => error.name === 'AbortError')
  assert.equal(client.current(), null, 'invalid updates cannot leave the old authorization usable')
  const mutable = props(), updated = new WorkOrdersClient()
  updated.update(mutable)
  const captured = updated.capture()
  mutable.signal = new AbortController().signal
  updated.update(mutable)
  assert.equal(updated.isCurrent(captured), false, 'signal identity is captured at acceptance instead of rereading mutable props')
  updated.stop()
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
      return delayedResponse(body.promise)
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

test('a stale 401 cannot invalidate a renewed context between transport rejection and client error handling', async () => {
  for (const queuedTurns of [1, 2]) {
    let invalidations = 0
    const client = new WorkOrdersClient({ onInvalidate: () => { invalidations++ },
      fetchImpl: async () => new Response('old-context-authentication-denied', { status: 401 }) })
    client.update(props())
    const next = props({ contextEpoch: 2, contextVersion: 'ctx-2', token: 'renewed-app-token' })
    const pending = client.list({}).then(() => null, error => error)
    const renew = () => client.update(next)
    if (queuedTurns === 1) queueMicrotask(renew)
    else queueMicrotask(() => queueMicrotask(renew))
    const error = await pending
    assert.equal(client.current(), next, `renewed context survives after ${queuedTurns} queued microtasks`)
    assert.equal(error?.name, 'AbortError')
    assert.equal(invalidations, 2, 'only the initial and renewed context transitions clear state')
    client.stop()
  }
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
    requests.push({ url, body: JSON.parse(options.body), method: options.method }); return new Response(JSON.stringify(publication()))
  } })
  client.update(props()); await client.remind('ticket-1', 'ticket:ticket-1:manual:event')
  assert.deepEqual(requests, [{ url: '/work-orders-api/tickets/ticket-1/remind', method: 'POST', body: { idempotencyKey: 'ticket:ticket-1:manual:event' } }])
  client.stop()
})


test('shared registered SDK distinguishes permission, conflict, network and protocol errors without echoing credentials', async () => {
  for (const [response, kind, status] of [[() => new Response('mock-app-token', { status: 403 }), 'FORBIDDEN', 403],
    [() => new Response('mock-app-token', { status: 409 }), 'CONFLICT', 409],
    [() => { throw new Error('mock-app-token') }, 'NETWORK', 503], [() => new Response('mock-app-token'), 'PROTOCOL', 502]]) {
    const client = new WorkOrdersClient({ fetchImpl: response })
    assert.equal(client.sdkVersion, '1.0.0'); client.update(props())
    await assert.rejects(client.list({}), (error) => {
      assert.ok(error instanceof BusinessError); assert.equal(error.kind, kind); assert.equal(error.status, status)
      assert.match(error.requestId, /^[0-9a-f-]{36}$/i); assert.equal(String(error).includes('mock-app-token'), false)
      return true
    })
    client.stop()
  }
})

test('Rust create uses one required Idempotency-Key header and never puts the key into its strict DTO', async () => {
  const requests = [], client = new WorkOrdersClient({ fetchImpl: async (url, options) => {
    requests.push({ url, ...options }); return new Response(JSON.stringify(serverTicket()), { status: 201 })
  } })
  client.update(props())
  await assert.rejects(client.create({ title: 'draft' }), TypeError)
  assert.equal(requests.length, 0)
  await client.create({ title: 'draft' }, 'create:event-one')
  assert.equal(requests.length, 1)
  assert.equal(requests[0].headers.get('Idempotency-Key'), 'create:event-one')
  assert.deepEqual(JSON.parse(requests[0].body), { title: 'draft' })
  assert.equal(requests[0].credentials, 'omit')
  client.stop()
})

test('unknown mutation responses retain typed outcomes without automatic retries or remote error text', async () => {
  for (const operation of ['create', 'update', 'remove', 'remind']) {
    for (const response of [() => { throw Error('mock-app-token') }, () => new Response('broken'),
      () => new Response('{"code":"ANYTHING","message":"mock-app-token"}', { status: 503 })]) {
      let requests = 0
      const client = new WorkOrdersClient({ fetchImpl: () => { requests++; return response() } })
      client.update(props())
      const pending = operation === 'create' ? client.create({ title: 'draft' }, 'same-key')
        : operation === 'update' ? client.updateTicket('ticket-1', { version: 1, title: 'draft' })
        : operation === 'remove' ? client.remove('ticket-1', 1) : client.remind('ticket-1', 'same-key')
      await assert.rejects(pending, error => {
        assert.equal(error.outcome, operation === 'create' ? 'CREATE_RESULT_UNKNOWN' : operation === 'remind' ? 'REMINDER_RESULT_UNKNOWN' : 'WRITE_RESULT_UNKNOWN')
        assert.equal(String(error).includes('mock-app-token'), false)
        return true
      })
      assert.equal(requests, 1); client.stop()
    }
  }
})

test('only bounded allowlisted Rust 503 codes can identify reminder preparation before platform publication', async () => {
  for (const [body, outcome] of [[JSON.stringify({ code: 'REMINDER_PREPARATION_UNKNOWN', message: 'mock-app-token' }), 'REMINDER_PREPARATION_UNKNOWN'],
    [JSON.stringify({ code: 'REMINDER_RESULT_UNKNOWN' }), 'REMINDER_RESULT_UNKNOWN'],
    [JSON.stringify({ code: 'REMINDER_PREPARATION_UNKNOWN', padding: 'x'.repeat(9000) }), 'REMINDER_RESULT_UNKNOWN'],
    ['{"code":"UNTRUSTED_REMOTE_TEXT"}', 'REMINDER_RESULT_UNKNOWN'], ['broken', 'REMINDER_RESULT_UNKNOWN']]) {
    const client = new WorkOrdersClient({ fetchImpl: async () => new Response(body, { status: 503 }) })
    client.update(props())
    await assert.rejects(client.remind('ticket-1', 'event'), error => {
      assert.equal(error.outcome, outcome)
      if (outcome === 'REMINDER_PREPARATION_UNKNOWN') assert.equal(error.code, outcome)
      assert.equal(String(error).includes('mock-app-token'), false)
      return true
    })
    client.stop()
  }
})

test('a deterministic reminder rejection does not claim an unknown publication and 401 remains immediate', async () => {
  for (const status of [400, 403, 404, 409]) {
    const client = new WorkOrdersClient({ fetchImpl: async () => new Response('{"code":"REMINDER_PREPARATION_UNKNOWN"}', { status }) })
    client.update(props()); await assert.rejects(client.remind('ticket-1', 'event'), error => error.status === status && error.outcome === undefined)
    client.stop()
  }
  const never = new ReadableStream({ start() {} })
  const client = new WorkOrdersClient({ fetchImpl: async () => new Response(never, { status: 401 }) })
  client.update(props()); await assert.rejects(client.remind('ticket-1', 'event'), error => error.status === 401)
  assert.equal(client.current(), null)
})

test('explicit creation recovery uses captured key and complete body after a lost committed response and renewal', async () => {
  const requests = [], intents = new IntentStore(), original = props(), draft = { title: 'Original', description: 'full body', priority: 'HIGH', assigneeUserId: null }
  let loseResponse = true
  const client = new WorkOrdersClient({ fetchImpl: async (url, options) => {
    requests.push({ url, key: options.headers.get('Idempotency-Key'), body: options.body, authorization: options.headers.get('Authorization') })
    if (loseResponse) { loseResponse = false; throw Error('connection lost after commit') }
    return new Response(JSON.stringify(serverTicket()), { status: 201 })
  } })
  client.update(original)
  const intent = intents.prepareCreate(original, draft)
  await assert.rejects(client.create(intent.body, intent.key), error => error.outcome === 'CREATE_RESULT_UNKNOWN')
  draft.title = 'Must not replace the event'
  const renewed = props({ contextEpoch: 2, contextVersion: 'ctx-2', token: 'renewed-app-token' })
  client.update(renewed)
  assert.equal(intents.getCreate(renewed), intent)
  assert.equal(requests.length, 1, 'renewal does not dispatch recovery')
  assert.deepEqual(await client.create(intent.body, intent.key), serverTicket())
  assert.equal(requests.length, 2)
  assert.equal(requests[0].key, requests[1].key); assert.equal(requests[0].body, requests[1].body)
  assert.equal(requests[1].authorization, 'Bearer renewed-app-token')
  intents.settleCreate(renewed, intent); assert.equal(intents.getCreate(renewed), null)
  client.stop()
})

test('context cancellation while reading Rust recovery metadata rejects late data and leaves the original event intact', async () => {
  const host = new AbortController(), original = props({ signal: host.signal }), intents = new IntentStore()
  let readBody
  const bodyStarted = deferred()
  const client = new WorkOrdersClient({ fetchImpl: async () => new Response(new ReadableStream({ start(controller) {
    readBody = controller; bodyStarted.resolve()
  } }), { status: 503 }) })
  client.update(original)
  const intent = intents.prepareReminder(original, 'ticket-1')
  const pending = client.remind(intent.ticketId, intent.key).then(value => ({ value }), error => ({ error }))
  await bodyStarted.promise; host.abort()
  const renewed = props({ contextEpoch: 2, contextVersion: 'ctx-2', token: 'renewed' })
  client.update(renewed)
  try { readBody.enqueue(new TextEncoder().encode('{"code":"REMINDER_PREPARATION_UNKNOWN"}')); readBody.close() } catch {}
  assert.equal((await pending).error?.name, 'AbortError')
  assert.equal(intents.getReminder(renewed, 'ticket-1'), intent)
  client.stop()
})

test('malformed successful mutation DTOs are unknown protocol outcomes and cannot erase captured events', async () => {
  for (const [operation, status, invalid] of [['create', 201, [{}, null, [], serverTicket({ id: 'invalid' }), serverTicket({ version: '1' }), serverTicket({ version: 2 }), serverTicket({ status: 'RESOLVED' })]],
    ['update', 200, [{}, serverTicket({ version: 0 }), serverTicket({ priority: 'constructor' })]],
    ['remove', 200, [{}, { ok: false }, { ok: 'true' }]],
    ['remind', 200, [{}, publication({ publicationId: 'invalid' }), publication({ recipientCount: '1' }), publication({ deliveryCount: 2 }), publication({ duplicate: 1 }), publication({ createdAt: 'tomorrow' })]]]) {
    for (const body of invalid) {
      const intents = new IntentStore(), original = props(), intent = intents.prepareCreate(original, { title: 'keep original' })
      let calls = 0
      const client = new WorkOrdersClient({ fetchImpl: async () => { calls++; return new Response(JSON.stringify(body), { status }) } })
      client.update(original)
      const pending = operation === 'create' ? client.create(intent.body, intent.key) : operation === 'update'
        ? client.updateTicket('ticket-1', { title: 'draft', version: 1 }) : operation === 'remove'
          ? client.remove('ticket-1', 1) : client.remind('ticket-1', 'event')
      await assert.rejects(pending, error => error instanceof BusinessError && error.kind === 'PROTOCOL' &&
        error.outcome === (operation === 'create' ? 'CREATE_RESULT_UNKNOWN' : operation === 'remind' ? 'REMINDER_RESULT_UNKNOWN' : 'WRITE_RESULT_UNKNOWN'))
      assert.equal(calls, 1); assert.equal(intents.getCreate(original), intent)
      client.stop()
    }
  }
})

test('valid mutation JSON with an unexpected success status cannot confirm a committed outcome', async () => {
  for (const [operation, body] of [['create', serverTicket()], ['update', serverTicket({ version: 2 })], ['remove', { ok: true }], ['remind', publication()]]) {
    const client = new WorkOrdersClient({ fetchImpl: async () => new Response(JSON.stringify(body), { status: 202 }) })
    client.update(props())
    await assert.rejects(operation === 'create' ? client.create({ title: 'draft' }, 'event') : operation === 'update'
      ? client.updateTicket('ticket-1', { version: 1, title: 'draft' }) : operation === 'remove'
        ? client.remove('ticket-1', 1) : client.remind('ticket-1', 'event'), error => error.kind === 'PROTOCOL' && !!error.outcome)
    client.stop()
  }
})

test('successful Rust DTO projections confirm real values and drop unrelated server fields', async () => {
  const record = serverTicket(), published = publication(), requests = []
  const client = new WorkOrdersClient({ fetchImpl: async (url, options) => {
    requests.push({ url, method: options.method })
    if (options.method === 'DELETE') return new Response('{"ok":true,"extra":"discard"}')
    if (url.endsWith('/remind')) return new Response(JSON.stringify({ ...published, extra: 'discard' }))
    return new Response(JSON.stringify({ ...record, version: options.method === 'PATCH' ? 2 : 1, extra: 'discard' }), { status: options.method === 'POST' ? 201 : 200 })
  } })
  client.update(props())
  assert.deepEqual(await client.create({ title: 'Original' }, 'event'), record)
  assert.deepEqual(await client.detail(record.id), record)
  assert.deepEqual(await client.updateTicket(record.id, { version: 1, title: 'Original' }), { ...record, version: 2 })
  assert.deepEqual(await client.remind(record.id, 'event'), published)
  assert.deepEqual(await client.remove(record.id, 1), { ok: true })
  assert.equal(requests.length, 5)
  client.stop()
})

test('malformed list/detail/member successes fail closed and cannot enter server-derived view state', async () => {
  const id = serverTicket().id
  for (const [operation, invalid] of [['detail', [{}, serverTicket({ id: publication().publicationId })]],
    ['list', [{}, { records: [serverTicket()], total: 1, page: 0, size: 20 }, { records: [{}], total: 1, page: 1, size: 20 }, { records: [], total: '0', page: 1, size: 20 }]],
    ['assignees', [{ records: [{}], total: 1, page: 1, size: 50 }, { records: [], total: 0, page: 1, size: 51 }]]]) {
    for (const body of invalid) {
      const client = new WorkOrdersClient({ fetchImpl: async () => new Response(JSON.stringify(body)) })
      client.update(props())
      await assert.rejects(operation === 'detail' ? client.detail(id) : operation === 'list'
        ? client.list({}) : client.assignees('create', serverTicket().organizationId), error => error.kind === 'PROTOCOL' && error.outcome === undefined)
      client.stop()
    }
  }
})
