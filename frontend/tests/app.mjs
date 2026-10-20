import assert from 'node:assert/strict'
import test from 'node:test'
import vm from 'node:vm'
import { fileURLToPath } from 'node:url'
import { webcrypto } from 'node:crypto'
import { build } from 'vite'

// Unit fixtures only: run the actual bundled application and its rendered
// handlers. These controlled HTTP replies are not service or browser acceptance.
const bundle = await build({ configFile: false, logLevel: 'silent', build: { write: false,
  lib: { entry: fileURLToPath(new URL('../src/app.mjs', import.meta.url)), name: 'UnitApp', formats: ['iife'] },
  rolldownOptions: { external: ['vue'], output: { globals: { vue: '__UNIT_VUE__' } } } } })
const appCode = (Array.isArray(bundle) ? bundle.flatMap(result => result.output) : bundle.output).find(file => file.type === 'chunk').code

const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done }); return { promise, resolve } }
const reply = (status, body) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
const deny = (status) => reply(status, { code: 'DENIED', message: '当前操作未获授权' })
const ticket = (id = '00000000-0000-4000-8000-000000000001', extra = {}) => ({ id, number: id === '00000000-0000-4000-8000-000000000001' ? 'WO-000001' : 'WO-000002',
  tenantId: '10000000-0000-4000-8000-000000000001', organizationId: '20000000-0000-4000-8000-000000000001', ownerUserId: '30000000-0000-4000-8000-000000000001', assigneeUserId: '30000000-0000-4000-8000-000000000002',
  organizationName: '测试组织', ownerDisplayName: '创建人', assigneeDisplayName: '处理人甲',
  title: id === '00000000-0000-4000-8000-000000000001' ? '服务器返回的工单甲' : '服务器返回的工单乙', description: '服务端原始描述',
  priority: 'NORMAL', status: 'OPEN', version: 1, createdAt: '2026-10-02T01:00:00.000Z', updatedAt: '2026-10-02T01:00:00.000Z', ...extra })
const directory = { records: [{ userId: '30000000-0000-4000-8000-000000000002', displayName: '处理人甲', organizationId: '20000000-0000-4000-8000-000000000001' }], total: 75, page: 1, size: 50 }

function context(epoch = 1, routePath = '/micro/work-orders', extra = {}) {
  const actions = ['read', 'create', 'update', 'delete', 'export']
  return { contractVersion: '2.0', appCode: 'work-orders', basePath: '/micro/work-orders', routePath,
    contextVersion: 'fixture-context-' + epoch, contextEpoch: epoch, token: 'fixture-app-token-' + epoch,
    expiresAt: new Date(Date.now() + 60_000).toISOString(), tokenExpiresAt: new Date(Date.now() + 60_000).toISOString(),
    signal: new AbortController().signal, user: { id: '30000000-0000-4000-8000-000000000001', displayName: '创建人' },
    tenant: { id: '10000000-0000-4000-8000-000000000001', name: '测试租户' }, organization: { id: '20000000-0000-4000-8000-000000000001', name: '测试组织' }, theme: 'light',
    permissions: actions.map((action) => 'ticket:' + action),
    dataGrants: actions.map((action) => ({ resource: 'ticket', action, scope: 'SELF', subjectUserId: '30000000-0000-4000-8000-000000000001', tenantId: '10000000-0000-4000-8000-000000000001', organizationIds: ['20000000-0000-4000-8000-000000000001'] })),
    onNavigate() {}, onNotify() {}, ...extra }
}

function h(type, props, children) {
  if (arguments.length === 2 && (props == null || typeof props !== 'object' || Array.isArray(props) || props.unitVNode)) {
    children = props; props = {}
  }
  return { unitVNode: true, type, props: props || {}, children: [children].flat(Infinity).filter((child) => child !== null && child !== undefined && child !== false) }
}
function* walk(node) {
  if (!node?.unitVNode) return
  yield node
  for (const child of node.children) yield* walk(child)
}
function textOf(node) {
  if (node == null || node === false) return ''
  return node.unitVNode ? node.children.map(textOf).join(' ') : String(node)
}
async function settle() { for (let count = 0; count < 3; count++) await new Promise(setImmediate) }

function harness(rows = [ticket()]) {
  let render, lifecycle, state
  const requests = [], responses = [], failures = [], container = { textContent: '' }
  const vue = { h, reactive: (value) => { state = value; return value }, nextTick: (callback) => Promise.resolve().then(callback),
    createApp: (component) => ({ mount() { render = component.render; render() }, unmount() { render = null } }) }
  const browser = { __WORK_ORDERS_CONFIG__: { version: 'unit-fixture', allowedHostOrigins: ['https://host.test'], apiBase: '/work-orders-api' } }
  const fetch = async (url, options) => {
    const request = { url: new URL(url, 'https://app.test'), method: options.method, body: options.body ? JSON.parse(options.body) : undefined,
      signal: options.signal, headers: options.headers }
    requests.push(request)
    const index = responses.findIndex((item) => item.method === request.method && item.path === request.url.pathname)
    if (index >= 0) return await responses.splice(index, 1)[0].response
    if (request.method === 'GET' && request.url.pathname === '/work-orders-api/tickets') return reply(200, { records: rows, total: rows.length, page: 1, size: 20 })
    if (request.method === 'GET' && request.url.pathname === '/work-orders-api/assignees') return reply(200, directory)
    if (request.method === 'GET') {
      const record = rows.find((row) => request.url.pathname === '/work-orders-api/tickets/' + row.id)
      if (record) return reply(200, record)
    }
    failures.push(request.method + ' ' + request.url.pathname)
    return reply(500, { message: '未配置此单元测试响应' })
  }
  const scope = { __UNIT_VUE__: vue, window: browser, document: { querySelector: (selector) => selector === '#app' ? container : { focus() {} },
    documentElement: { dataset: {} } }, fetch, AbortController, DOMException, Headers, TextEncoder, TextDecoder, URLSearchParams, Uint8Array,
    crypto: webcrypto, console }
  vm.runInNewContext(appCode, scope, { filename: 'actual-work-orders-unit-bundle.js' })
  assert.equal(typeof scope.UnitApp?.createLifecycle, 'function', 'actual UI chunk exports its lifecycle factory')
  lifecycle = scope.UnitApp.createLifecycle({ downloadCsv: async () => {} })
  const api = {
    get state() { return state }, requests, failures,
    enqueue(method, path, response) { responses.push({ method, path: '/work-orders-api' + path, response }) },
    tree() { return render() },
    nodes(testId) { return [...walk(render())].filter((node) => node.props['data-testid'] === testId) },
    node(testId) { const node = api.nodes(testId)[0]; assert.ok(node, 'rendered ' + testId + ' exists'); return node },
    text() { return textOf(render()) },
    async mount(props = context()) { lifecycle.mount(container, props); await settle() },
    async update(props) { lifecycle.update(props); await settle() },
    async click(testId) { const node = api.node(testId); assert.equal(!!node.props.disabled, false, testId + ' is enabled'); await node.props.onClick(); await settle() },
    input(testId, value) { const node = api.node(testId); assert.equal(!!node.props.disabled, false); node.props.onInput({ target: { value } }) },
    async submit() { const editor = api.node('ticket-editor'); const form = [...walk(editor)].find((node) => node.type === 'form');
      assert.ok(form); form.props.onSubmit({ preventDefault() {} }); await settle() },
    unmount() { lifecycle.unmount() },
  }
  return api
}

test('actual app unit: create navigation and direct entry use the separate creation route', async t => {
  const navigations = [], app = harness(); t.after(() => app.unmount())
  const initial = context(1, '/micro/work-orders', { onNavigate: path => navigations.push(path) })
  await app.mount(initial); await app.click('create-ticket')
  assert.equal(navigations.at(-1), '/micro/work-orders/ticket-create')
  assert.equal(app.state.editor.kind, 'create')
  const direct = harness(); t.after(() => direct.unmount())
  await direct.mount({ ...initial, routePath: '/micro/work-orders/ticket-create' })
  assert.equal(direct.state.editor.kind, 'create')
  assert.equal(app.requests.some(request => request.url.pathname.endsWith('/tickets/new')), false)
  assert.deepEqual(app.failures, [])
})

test('actual app unit: legacy new URL remains a read and never opens a creation editor', async t => {
  const app = harness(); t.after(() => app.unmount())
  app.enqueue('GET', '/tickets/new', deny(400))
  const initial = context(1, '/micro/work-orders/tickets/new', { permissions: ['ticket:read'],
    dataGrants: context().dataGrants.filter(grant => grant.action === 'read') })
  await app.mount(initial)
  assert.equal(app.state.editor, null)
  assert.equal(app.state.mode, 'detail')
  assert.equal(app.state.detail, null, 'the Rust API rejects a non UUID detail id')
  assert.equal(app.requests.some(request => request.method === 'GET' && request.url.pathname.endsWith('/tickets/new')), true)
  assert.equal(app.requests.some(request => request.method === 'POST'), false)
  assert.equal(app.nodes('create-ticket').length, 0)
  assert.deepEqual(app.failures, [])
})

test('actual app unit: HTTP 401 invalidates the app and clears a cached ticket and open delete confirmation', async (t) => {
  const app = harness(); t.after(() => app.unmount())
  await app.mount(); await app.click('open-ticket'); await app.click('delete-ticket')
  app.enqueue('DELETE', '/tickets/00000000-0000-4000-8000-000000000001', deny(401))
  await app.click('confirm-delete')
  assert.equal(app.node('work-orders-app').props['data-context-active'], 'false')
  assert.equal(app.nodes('ticket-detail').length, 0)
  assert.equal(app.nodes('delete-confirmation').length, 0)
  assert.equal(app.state.rows.length, 0); assert.equal(app.state.detail, null); assert.equal(app.state.deleteTarget, null)
  assert.doesNotMatch(app.text(), /服务器返回的工单甲/)
  assert.deepEqual(app.failures, [])
})

test('actual app unit: theme updates retain a pending submit and do not reload cached lists', async t => {
  const app = harness(), initial = context(), pending = deferred(); t.after(() => app.unmount())
  await app.mount(initial)
  await app.click('create-ticket'); app.input('ticket-title', '正在提交的草稿')
  app.enqueue('POST', '/tickets', pending.promise)
  await app.submit()
  const write = app.requests.find(request => request.method === 'POST')
  const reads = app.requests.filter(request => request.method === 'GET' && request.url.pathname.endsWith('/tickets')).length
  await app.update({ ...initial, theme: 'dark' })
  assert.equal(write.signal.aborted, false)
  assert.equal(app.state.busy.save, true)
  assert.equal(app.requests.filter(request => request.method === 'GET' && request.url.pathname.endsWith('/tickets')).length, reads)
  pending.resolve(reply(201, ticket()))
  await settle()
  assert.equal(app.state.detail?.id, '00000000-0000-4000-8000-000000000001')
  assert.equal(app.state.editor, null)
})

test('actual app unit: a display update cannot clear a server denial or resubmit a forbidden draft', async t => {
  const app = harness(), initial = context(); t.after(() => app.unmount())
  await app.mount(initial); await app.click('create-ticket'); app.input('ticket-title', '保留的草稿')
  app.enqueue('POST', '/tickets', deny(403)); await app.submit()
  assert.equal(app.state.deniedActions.has('ticket:create'), true)
  await app.update({ ...initial, theme: 'dark' })
  assert.equal(app.state.deniedActions.has('ticket:create'), true)
  assert.equal(app.node('save-ticket').props.disabled, true)
})

test('actual app unit: removing read authorization clears sensitive editor snapshots despite an unchanged context hash', async t => {
  const app = harness(), initial = context(); t.after(() => app.unmount())
  await app.mount(initial); await app.click('open-ticket'); await app.click('edit-ticket')
  await app.update({ ...initial, permissions: ['ticket:update'], dataGrants: initial.dataGrants.filter(grant => grant.action === 'update') })
  assert.equal(app.state.detail, null)
  assert.equal(app.state.editor, null)
  assert.equal(app.state.assignees.length, 0)
  assert.equal(app.state.deleteTarget, null)
})

test('actual app unit: route changes cancel related reads without cancelling an independent pending write', async t => {
  const app = harness([ticket(), ticket('00000000-0000-4000-8000-000000000002')]), initial = context(1, '/micro/work-orders/tickets/00000000-0000-4000-8000-000000000001')
  const write = deferred(), detail = deferred(); t.after(() => app.unmount())
  await app.mount(initial); await app.click('edit-ticket'); app.input('ticket-title', '独立写入')
  app.enqueue('PATCH', '/tickets/00000000-0000-4000-8000-000000000001', write.promise); await app.submit()
  const saving = app.requests.find(request => request.method === 'PATCH')
  app.enqueue('GET', '/tickets/00000000-0000-4000-8000-000000000002', detail.promise)
  await app.update({ ...initial, routePath: '/micro/work-orders/tickets/00000000-0000-4000-8000-000000000002' })
  const reading = app.requests.find(request => request.url.pathname.endsWith('/00000000-0000-4000-8000-000000000002'))
  assert.equal(saving.signal.aborted, false)
  await app.update({ ...initial, routePath: '/micro/work-orders' })
  assert.equal(reading.signal.aborted, true)
  assert.equal(saving.signal.aborted, false)
  assert.equal(app.requests.filter(request => request.method === 'GET' && request.url.pathname.endsWith('/tickets')).length, 1)
  detail.resolve(reply(200, ticket('00000000-0000-4000-8000-000000000002'))); await settle()
  assert.equal(app.state.mode, 'list'); assert.equal(app.state.detail, null)
  write.resolve(reply(200, ticket('00000000-0000-4000-8000-000000000001', { title: '独立写入', version: 2 }))); await settle()
  assert.equal(app.state.mode, 'list'); assert.equal(app.state.detail, null)
  assert.equal(app.state.busy.save, false)
})

test('actual app unit: host cancellation after a theme update clears server snapshots while retaining only the draft', async t => {
  const source = new AbortController(), app = harness(), initial = context(1, '/micro/work-orders/tickets/00000000-0000-4000-8000-000000000001', { signal: source.signal })
  t.after(() => app.unmount())
  await app.mount(initial); await app.click('edit-ticket'); app.input('ticket-title', '我的草稿')
  await app.update({ ...initial, theme: 'dark' })
  source.abort(); await settle()
  assert.equal(app.state.active, false)
  assert.equal(app.state.detail, null); assert.equal(app.state.rows.length, 0)
  assert.equal(app.state.editor.ticket, null); assert.equal(app.state.editor.latest, null)
  assert.equal(app.state.assignees.length, 0); assert.equal(app.state.deleteTarget, null)
  assert.equal(app.state.editor.draft.title, '我的草稿')
})

test('actual app unit: list 403 removes cached rows and blocks another read until fresh props arrive', async (t) => {
  const app = harness(); t.after(() => app.unmount())
  await app.mount(); assert.equal(app.nodes('ticket-row').length, 1)
  app.enqueue('GET', '/tickets', deny(403))
  await app.click('refresh-tickets')
  assert.equal(app.state.rows.length, 0); assert.equal(app.state.total, 0); assert.equal(app.state.detail, null)
  assert.equal(app.nodes('ticket-row').length, 0)
  assert.equal(!!app.node('refresh-tickets').props.disabled, true)
  const count = app.requests.length
  app.node('filter-status').props.onChange({ target: { value: 'OPEN' } }); await settle()
  assert.equal(app.requests.length, count, 'the actual filter handler cannot read again under denied props')
  await app.update(context(2))
  assert.equal(!!app.node('refresh-tickets').props.disabled, false)
  assert.equal(app.nodes('ticket-row').length, 1)
  assert.deepEqual(app.failures, [])
})

test('actual app unit: update 409 followed by detail 404 drops server caches and keeps the user draft', async (t) => {
  const app = harness(); t.after(() => app.unmount())
  await app.mount(); await app.click('open-ticket'); await app.click('edit-ticket')
  app.input('ticket-title', '我尚未提交的修改')
  app.enqueue('PATCH', '/tickets/00000000-0000-4000-8000-000000000001', reply(409, { code: 'VERSION_CONFLICT', message: '版本冲突' }))
  app.enqueue('GET', '/tickets/00000000-0000-4000-8000-000000000001', deny(404))
  await app.submit()
  assert.equal(app.state.detail, null)
  assert.equal(app.state.rows.some((row) => row.id === '00000000-0000-4000-8000-000000000001'), false)
  assert.equal(app.state.editor.latest, null)
  assert.equal(app.node('ticket-title').props.value, '我尚未提交的修改')
  assert.equal(!!app.node('use-latest-version').props.disabled, true)
  assert.equal(app.nodes('detail-number').length, 0)
  assert.deepEqual(app.failures, [])
})

test('actual app unit: context renewal clears an earlier conflict latest before a deferred refresh', async (t) => {
  const app = harness(), pending = deferred(); t.after(() => app.unmount())
  await app.mount(); await app.click('open-ticket'); await app.click('edit-ticket')
  app.input('ticket-title', '保留的输入')
  app.enqueue('PATCH', '/tickets/00000000-0000-4000-8000-000000000001', reply(409, { message: '版本冲突' }))
  app.enqueue('GET', '/tickets/00000000-0000-4000-8000-000000000001', reply(200, ticket('00000000-0000-4000-8000-000000000001', { version: 2, title: '服务端较新标题' })))
  await app.submit()
  assert.equal(!!app.node('use-latest-version').props.disabled, false)
  app.enqueue('GET', '/tickets/00000000-0000-4000-8000-000000000001', pending.promise)
  await app.update(context(2, '/micro/work-orders/tickets/00000000-0000-4000-8000-000000000001'))
  assert.equal(app.state.editor.latest, null)
  assert.equal(!!app.node('use-latest-version').props.disabled, true)
  assert.equal(app.node('ticket-title').props.value, '保留的输入')
  pending.resolve(deny(404)); await settle()
  assert.equal(app.state.detail, null); assert.equal(app.state.editor.latest, null)
  assert.equal(!!app.node('use-latest-version').props.disabled, true)
  assert.deepEqual(app.failures, [])
})

test('actual app unit: a late detail denial cannot clear a different ticket opened in the same context', async (t) => {
  const app = harness([ticket(), ticket('00000000-0000-4000-8000-000000000002')]), pending = deferred(); t.after(() => app.unmount())
  await app.mount(); await app.click('open-ticket'); await app.click('edit-ticket')
  app.enqueue('PATCH', '/tickets/00000000-0000-4000-8000-000000000001', reply(409, { message: '版本冲突' }))
  app.enqueue('GET', '/tickets/00000000-0000-4000-8000-000000000001', pending.promise)
  await app.submit()
  const cancel = [...walk(app.node('ticket-editor'))].find((node) => node.type === 'button' && textOf(node) === '取消')
  assert.ok(cancel); cancel.props.onClick(); await settle()
  await app.click('back-to-list')
  const secondRow = app.nodes('ticket-row').find((row) => row.props['data-ticket-id'] === '00000000-0000-4000-8000-000000000002')
  const openSecond = [...walk(secondRow)].find((node) => node.props['data-testid'] === 'open-ticket')
  openSecond.props.onClick(); await settle()
  pending.resolve(deny(404)); await settle()
  assert.equal(app.state.detail.id, '00000000-0000-4000-8000-000000000002')
  assert.equal(app.node('detail-number').children[0], 'WO-000002')
  assert.equal(app.state.rows.length, 2, 'a retired target denial does not alter the current list cache')
  assert.deepEqual(app.failures, [])
})

test('actual app unit: directory 403 clears candidates and locks assignment while retaining other draft fields', async (t) => {
  const app = harness(); t.after(() => app.unmount())
  await app.mount(); await app.click('open-ticket'); await app.click('edit-ticket')
  app.input('ticket-title', '目录被拒绝时保留标题'); app.input('ticket-description', '目录被拒绝时保留描述')
  app.enqueue('GET', '/assignees', deny(403))
  await app.click('next-assignees')
  assert.equal(app.state.assignees.length, 0); assert.equal(app.state.assigneeTotal, 0)
  assert.equal(!!app.node('ticket-assignee').props.disabled, true)
  assert.equal(app.node('ticket-title').props.value, '目录被拒绝时保留标题')
  assert.equal(app.node('ticket-description').props.value, '目录被拒绝时保留描述')
  assert.equal(!!app.node('save-ticket').props.disabled, false, 'directory denial does not deny ordinary ticket fields')
  app.enqueue('PATCH', '/tickets/00000000-0000-4000-8000-000000000001', reply(200, ticket('00000000-0000-4000-8000-000000000001', { title: '目录被拒绝时保留标题', version: 2 })))
  await app.submit()
  const written = app.requests.findLast((request) => request.method === 'PATCH')
  assert.equal(Object.hasOwn(written.body, 'assigneeUserId'), false, 'locked assignment is omitted from the actual PATCH')
  assert.equal(written.body.title, '目录被拒绝时保留标题')
  assert.deepEqual(app.failures, [])
})

for (const kind of ['create', 'update']) {
  test('actual app unit: ' + kind + ' 403 retains its draft, prevents resubmission, and fresh props restore the action', async (t) => {
    const app = harness(); t.after(() => app.unmount())
    await app.mount()
    if (kind === 'create') await app.click('create-ticket')
    else { await app.click('open-ticket'); await app.click('edit-ticket') }
    app.input('ticket-title', '403 后仍在的 ' + kind + ' 草稿')
    const method = kind === 'create' ? 'POST' : 'PATCH', path = kind === 'create' ? '/tickets' : '/tickets/00000000-0000-4000-8000-000000000001'
    app.enqueue(method, path, deny(403))
    await app.submit()
    assert.equal(app.node('ticket-title').props.value, '403 后仍在的 ' + kind + ' 草稿')
    assert.equal(!!app.node('save-ticket').props.disabled, true)
    const count = app.requests.filter((request) => request.method === method).length
    await app.submit()
    assert.equal(app.requests.filter((request) => request.method === method).length, count, 'actual submit handler blocks a second denied write')
    await app.update(context(2, '/micro/work-orders/' + (kind === 'create' ? 'ticket-create' : 'tickets/00000000-0000-4000-8000-000000000001')))
    assert.equal(app.node('ticket-title').props.value, '403 后仍在的 ' + kind + ' 草稿')
    assert.equal(!!app.node('save-ticket').props.disabled, false)
    app.enqueue(method, path, reply(kind === 'create' ? 201 : 200, ticket('00000000-0000-4000-8000-000000000001', { title: '403 后仍在的 ' + kind + ' 草稿', version: kind === 'create' ? 1 : 2 })))
    await app.submit()
    assert.equal(app.nodes('ticket-editor').length, 0)
    assert.equal(app.state.detail.title, '403 后仍在的 ' + kind + ' 草稿')
    assert.deepEqual(app.failures, [])
  })
}

test('actual app unit: an unknown creation explicitly recovers the immutable original body and key', async t => {
  const app = harness(); t.after(() => app.unmount())
  await app.mount(); await app.click('create-ticket'); app.input('ticket-title', '可能已经提交的创建')
  app.enqueue('POST', '/tickets', reply(503, { code: 'CREATE_RESULT_UNKNOWN', message: '结果未知' }))
  await app.submit()
  const first = app.requests.find(request => request.method === 'POST')
  assert.ok(first.headers.get('Idempotency-Key'), 'creation uses the required stable header')
  assert.equal(app.node('ticket-title').props.disabled, true, 'the unknown event body is immutable')
  assert.match(app.text(), /原事件|原键/)
  app.enqueue('POST', '/tickets', reply(201, ticket('00000000-0000-4000-8000-000000000001', { title: '可能已经提交的创建' })))
  await app.click('recover-create')
  const writes = app.requests.filter(request => request.method === 'POST')
  assert.equal(writes.length, 2)
  assert.equal(writes[1].headers.get('Idempotency-Key'), first.headers.get('Idempotency-Key'))
  assert.deepEqual(writes[1].body, first.body)
  assert.equal(app.state.editor, null)
  assert.deepEqual(app.failures, [])
})

test('actual app unit: organization switches cancel old creation and isolate its recoverable event', async t => {
  const pending = deferred(), app = harness(), original = context(); t.after(() => app.unmount())
  await app.mount(original); await app.click('create-ticket'); app.input('ticket-title', '组织甲的创建')
  app.enqueue('POST', '/tickets', pending.promise); await app.submit()
  const first = app.requests.find(request => request.method === 'POST')
  const other = context(2, '/micro/work-orders/ticket-create', { organization: { id: '20000000-0000-4000-8000-000000000002', name: '组织乙' },
    dataGrants: original.dataGrants.map(grant => ({ ...grant, organizationIds: ['20000000-0000-4000-8000-000000000002'] })) })
  await app.update(other)
  assert.equal(first.signal.aborted, true)
  assert.equal(app.node('ticket-title').props.value, '')
  assert.equal(app.nodes('recover-create').length, 0)
  assert.doesNotMatch(app.text(), /组织甲的创建/)
  await app.update(context(3, '/micro/work-orders/ticket-create'))
  assert.equal(app.node('ticket-title').props.value, '组织甲的创建')
  assert.equal(app.nodes('recover-create').length, 1)
  assert.equal(app.requests.filter(request => request.method === 'POST').length, 1, 'restoring identity never sends automatically')
  pending.resolve(reply(201, ticket())); await settle()
  assert.equal(app.state.detail, null, 'an old response cannot expose server data after identity changes')
})

for (const code of ['REMINDER_PREPARATION_UNKNOWN', 'REMINDER_RESULT_UNKNOWN']) {
  test('actual app unit: ' + code + ' keeps the original key for explicit recovery without changing ticket version', async t => {
    const app = harness(), initial = context(1, '/micro/work-orders/tickets/00000000-0000-4000-8000-000000000001'); t.after(() => app.unmount())
    initial.permissions.push('notification:publish')
    initial.dataGrants.push({ resource: 'notification', action: 'publish', scope: 'ORG', subjectUserId: '30000000-0000-4000-8000-000000000001', tenantId: '10000000-0000-4000-8000-000000000001', organizationIds: ['20000000-0000-4000-8000-000000000001'] })
    await app.mount(initial)
    app.enqueue('POST', '/tickets/00000000-0000-4000-8000-000000000001/remind', reply(503, { code, message: '结果未知' }))
    await app.click('remind-ticket')
    const first = app.requests.find(request => request.method === 'POST')
    assert.match(app.text(), code === 'REMINDER_PREPARATION_UNKNOWN' ? /准备|准备阶段/ : /结果尚未确认/)
    assert.equal(app.state.detail.version, 1)
    await app.update({ ...initial, theme: 'dark' })
    assert.equal(app.requests.filter(request => request.method === 'POST').length, 1)
    app.enqueue('POST', '/tickets/00000000-0000-4000-8000-000000000001/remind', reply(200, { publicationId: '0805aef9-5c92-40b2-8db9-739577f20b1e', recipientCount: 1, deliveryCount: 1, duplicate: true, createdAt: '2026-10-17T00:00:00Z' }))
    await app.click('remind-ticket')
    const writes = app.requests.filter(request => request.method === 'POST')
    assert.deepEqual(writes[1].body, first.body)
    assert.equal(app.state.detail.version, 1)
    assert.match(app.text(), /此前的工单提醒已确认/)
    assert.deepEqual(app.failures, [])
  })
}

test('actual app unit: a forbidden replay does not discard an earlier unknown creation', async t => {
  const app = harness(); t.after(() => app.unmount())
  await app.mount(); await app.click('create-ticket'); app.input('ticket-title', '不可换键的事件')
  app.enqueue('POST', '/tickets', reply(503, { code: 'CREATE_RESULT_UNKNOWN' })); await app.submit()
  const first = app.requests.find(request => request.method === 'POST')
  app.enqueue('POST', '/tickets', deny(403)); await app.click('recover-create')
  assert.equal(app.node('ticket-title').props.disabled, true)
  assert.equal(app.node('recover-create').props.disabled, true)
  await app.update(context(2, '/micro/work-orders/ticket-create'))
  assert.equal(app.node('recover-create').props.disabled, false)
  app.enqueue('POST', '/tickets', reply(201, ticket())); await app.click('recover-create')
  const writes = app.requests.filter(request => request.method === 'POST')
  assert.equal(writes.length, 3)
  for (const write of writes) {
    assert.equal(write.headers.get('Idempotency-Key'), first.headers.get('Idempotency-Key'))
    assert.deepEqual(write.body, first.body)
  }
})

test('actual app unit: unknown edits require an explicit current read and version choice before another save', async t => {
  const app = harness(); t.after(() => app.unmount())
  await app.mount(); await app.click('open-ticket'); await app.click('edit-ticket'); app.input('ticket-title', '可能已保存的输入')
  app.enqueue('PATCH', '/tickets/00000000-0000-4000-8000-000000000001', reply(503, { code: 'WRITE_RESULT_UNKNOWN' })); await app.submit()
  assert.equal(app.node('ticket-title').props.value, '可能已保存的输入')
  assert.equal(app.node('save-ticket').props.disabled, true)
  assert.equal(app.requests.filter(request => request.url.pathname.endsWith('/00000000-0000-4000-8000-000000000001') && request.method === 'GET').length, 0,
    'the unknown write waits for an explicit reconciliation read')
  app.enqueue('GET', '/tickets/00000000-0000-4000-8000-000000000001', reply(200, ticket('00000000-0000-4000-8000-000000000001', { version: 2 })))
  await app.click('verify-write-result')
  assert.equal(app.node('save-ticket').props.disabled, true)
  await app.click('use-latest-version')
  assert.equal(app.node('save-ticket').props.disabled, false)
  assert.equal(app.node('ticket-title').props.value, '可能已保存的输入')
  app.enqueue('PATCH', '/tickets/00000000-0000-4000-8000-000000000001', reply(200, ticket('00000000-0000-4000-8000-000000000001', { version: 3 }))); await app.submit()
  assert.equal(app.requests.filter(request => request.method === 'PATCH').at(-1).body.version, 2)
  assert.deepEqual(app.failures, [])
})

test('actual app unit: an unknown deletion blocks a second delete until a current detail is explicitly checked', async t => {
  const app = harness(); t.after(() => app.unmount())
  await app.mount(); await app.click('open-ticket'); await app.click('delete-ticket')
  app.enqueue('DELETE', '/tickets/00000000-0000-4000-8000-000000000001', reply(503, { code: 'WRITE_RESULT_UNKNOWN' })); await app.click('confirm-delete')
  assert.equal(app.node('confirm-delete').props.disabled, true)
  assert.equal(app.requests.filter(request => request.method === 'DELETE').length, 1)
  app.enqueue('GET', '/tickets/00000000-0000-4000-8000-000000000001', deny(404)); await app.click('verify-delete-result')
  assert.equal(app.nodes('delete-confirmation').length, 0)
  assert.equal(app.state.detail, null)
  assert.equal(app.requests.filter(request => request.method === 'DELETE').length, 1)
  assert.deepEqual(app.failures, [])
})

test('actual app unit: creation-only entry does not expose read routes or export and read-only detail cannot create', async t => {
  const createProps = context(1, '/micro/work-orders/ticket-create')
  createProps.permissions = ['ticket:create']; createProps.dataGrants = createProps.dataGrants.filter(grant => grant.action === 'create')
  const create = harness(); t.after(() => create.unmount()); await create.mount(createProps)
  assert.equal(create.state.editor.kind, 'create')
  assert.equal(create.nodes('ticket-row').length, 0); assert.equal(create.nodes('export-tickets').length, 0)
  assert.equal(create.requests.some(request => request.url.pathname.includes('/tickets')), false)
  const readProps = context(1, '/micro/work-orders/tickets/00000000-0000-4000-8000-000000000001')
  readProps.permissions = ['ticket:read']; readProps.dataGrants = readProps.dataGrants.filter(grant => grant.action === 'read')
  const read = harness(); t.after(() => read.unmount()); await read.mount(readProps)
  assert.equal(read.state.detail.id, '00000000-0000-4000-8000-000000000001'); assert.equal(read.nodes('create-ticket').length, 0)
  assert.equal(read.nodes('edit-ticket').length, 0); assert.equal(read.nodes('remind-ticket').length, 0)
  assert.equal(read.requests.some(request => request.method !== 'GET'), false)
})

test('actual app unit: a forbidden reminder recovery survives renewal and still uses the original event', async t => {
  const app = harness(), initial = context(1, '/micro/work-orders/tickets/00000000-0000-4000-8000-000000000001'); t.after(() => app.unmount())
  initial.permissions.push('notification:publish')
  initial.dataGrants.push({ resource: 'notification', action: 'publish', scope: 'ORG', subjectUserId: '30000000-0000-4000-8000-000000000001', tenantId: '10000000-0000-4000-8000-000000000001', organizationIds: ['20000000-0000-4000-8000-000000000001'] })
  await app.mount(initial)
  app.enqueue('POST', '/tickets/00000000-0000-4000-8000-000000000001/remind', reply(503, { code: 'REMINDER_RESULT_UNKNOWN' })); await app.click('remind-ticket')
  app.enqueue('POST', '/tickets/00000000-0000-4000-8000-000000000001/remind', deny(403)); await app.click('remind-ticket')
  assert.equal(app.nodes('remind-ticket').length, 0, 'the current denial removes the action')
  assert.equal(app.state.reminders.size, 1, 'denial of this replay cannot settle the prior unknown result')
  await app.update({ ...initial, contextEpoch: 2, contextVersion: 'renewed', token: 'renewed-token', signal: new AbortController().signal })
  assert.equal(app.requests.filter(request => request.method === 'POST').length, 2)
  app.enqueue('POST', '/tickets/00000000-0000-4000-8000-000000000001/remind', reply(200, { publicationId: '0805aef9-5c92-40b2-8db9-739577f20b1e', recipientCount: 1, deliveryCount: 1, duplicate: true, createdAt: '2026-10-17T00:00:00Z' }))
  await app.click('remind-ticket')
  const writes = app.requests.filter(request => request.method === 'POST')
  assert.equal(writes.length, 3)
  for (const write of writes) assert.deepEqual(write.body, writes[0].body)
  assert.equal(app.state.reminders.size, 0)
})

test('actual app unit: unknown quick status changes wait for explicit readback before another mutation', async t => {
  const app = harness(); t.after(() => app.unmount()); await app.mount(); await app.click('open-ticket')
  app.enqueue('PATCH', '/tickets/00000000-0000-4000-8000-000000000001', reply(503, { code: 'WRITE_RESULT_UNKNOWN' })); await app.click('change-ticket-status')
  assert.equal(app.node('change-ticket-status').props.disabled, true)
  assert.equal(app.requests.filter(request => request.method === 'PATCH').length, 1)
  app.enqueue('GET', '/tickets/00000000-0000-4000-8000-000000000001', reply(200, ticket('00000000-0000-4000-8000-000000000001', { version: 2, status: 'IN_PROGRESS' })))
  await app.click('verify-status-result')
  assert.equal(app.state.detail.status, 'IN_PROGRESS'); assert.equal(app.state.detail.version, 2)
  assert.equal(app.node('change-ticket-status').props.disabled, false)
  assert.equal(app.requests.filter(request => request.method === 'PATCH').length, 1)
  assert.deepEqual(app.failures, [])
})

test('actual app unit: checking one unknown deletion cannot settle another ticket across navigation and renewal', async t => {
  const app = harness([ticket(), ticket('00000000-0000-4000-8000-000000000002')]), initial = context(1, '/micro/work-orders/tickets/00000000-0000-4000-8000-000000000001')
  t.after(() => app.unmount()); await app.mount(initial); await app.click('delete-ticket')
  app.enqueue('DELETE', '/tickets/00000000-0000-4000-8000-000000000001', reply(503, { code: 'WRITE_RESULT_UNKNOWN' })); await app.click('confirm-delete')
  await app.update(context(2, '/micro/work-orders/tickets/00000000-0000-4000-8000-000000000002'))
  await app.click('delete-ticket')
  assert.equal(app.node('confirm-delete').props.disabled, false, 'ticket A uncertainty cannot block ticket B')
  app.enqueue('DELETE', '/tickets/00000000-0000-4000-8000-000000000002', reply(503, { code: 'WRITE_RESULT_UNKNOWN' })); await app.click('confirm-delete')
  app.enqueue('GET', '/tickets/00000000-0000-4000-8000-000000000002', reply(200, ticket('00000000-0000-4000-8000-000000000002', { version: 2 }))); await app.click('verify-delete-result')
  assert.equal(app.node('confirm-delete').props.disabled, false)
  await app.update(context(3, '/micro/work-orders/tickets/00000000-0000-4000-8000-000000000001')); await app.click('delete-ticket')
  assert.equal(app.node('confirm-delete').props.disabled, true, 'checking B cannot clear the original unknown delete of A')
  assert.equal(app.nodes('verify-delete-result').length, 1)
  assert.equal(app.requests.filter(request => request.method === 'DELETE').length, 2)
  assert.deepEqual(app.failures, [])
})

test('actual app unit: malformed creation success cannot settle a potentially committed event', async t => {
  const app = harness(); t.after(() => app.unmount()); await app.mount(); await app.click('create-ticket')
  app.input('ticket-title', '合法JSON不等于创建成功')
  app.enqueue('POST', '/tickets', reply(201, {})); await app.submit()
  assert.equal(app.node('ticket-title').props.disabled, true)
  assert.equal(app.nodes('recover-create').length, 1)
  app.enqueue('POST', '/tickets', reply(201, ticket())); await app.click('recover-create')
  const writes = app.requests.filter(request => request.method === 'POST')
  assert.equal(writes[0].headers.get('Idempotency-Key'), writes[1].headers.get('Idempotency-Key'))
  assert.deepEqual(writes[0].body, writes[1].body)
  assert.equal(app.state.editor, null)
})

test('actual app unit: malformed reminder success retains its event and requires an explicit same key recovery', async t => {
  const app = harness(), initial = context(1, '/micro/work-orders/tickets/00000000-0000-4000-8000-000000000001'); t.after(() => app.unmount())
  initial.permissions.push('notification:publish')
  initial.dataGrants.push({ resource: 'notification', action: 'publish', scope: 'ORG', subjectUserId: '30000000-0000-4000-8000-000000000001', tenantId: '10000000-0000-4000-8000-000000000001', organizationIds: ['20000000-0000-4000-8000-000000000001'] })
  await app.mount(initial)
  app.enqueue('POST', '/tickets/00000000-0000-4000-8000-000000000001/remind', reply(200, {})); await app.click('remind-ticket')
  assert.equal(app.state.reminders.size, 1)
  assert.match(app.text(), /提醒结果尚未确认/)
  app.enqueue('POST', '/tickets/00000000-0000-4000-8000-000000000001/remind', reply(200, { publicationId: '0805aef9-5c92-40b2-8db9-739577f20b1e', recipientCount: 1, deliveryCount: 1, duplicate: true, createdAt: '2026-10-17T00:00:00Z' }))
  await app.click('remind-ticket')
  const writes = app.requests.filter(request => request.method === 'POST')
  assert.deepEqual(writes[0].body, writes[1].body)
  assert.equal(app.state.reminders.size, 0)
})
