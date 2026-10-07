import assert from 'node:assert/strict'
import test from 'node:test'
import vm from 'node:vm'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import { webcrypto } from 'node:crypto'

// Unit fixtures only: run the actual bundled application and its rendered
// handlers. These controlled HTTP replies are not service or browser acceptance.
const require = createRequire(import.meta.url)
let esbuild
try { esbuild = require('esbuild') }
catch { esbuild = require(fileURLToPath(new URL('../../../../frontend/node_modules/esbuild', import.meta.url))) }
const bundle = await esbuild.build({ entryPoints: [fileURLToPath(new URL('../src/app.mjs', import.meta.url))],
  bundle: true, format: 'iife', platform: 'browser', target: 'es2022', write: false })
const appCode = bundle.outputFiles[0].text

const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done }); return { promise, resolve } }
const reply = (status, body) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
const deny = (status) => reply(status, { code: 'DENIED', message: '当前操作未获授权' })
const ticket = (id = 'ticket-a', extra = {}) => ({ id, number: id === 'ticket-a' ? 'WO-000001' : 'WO-000002',
  tenantId: 'tenant-1', organizationId: 'org-1', ownerUserId: 'user-1', assigneeUserId: 'user-2',
  organizationName: '测试组织', ownerDisplayName: '创建人', assigneeDisplayName: '处理人甲',
  title: id === 'ticket-a' ? '服务器返回的工单甲' : '服务器返回的工单乙', description: '服务端原始描述',
  priority: 'NORMAL', status: 'OPEN', version: 1, createdAt: '2026-10-02T01:00:00.000Z', updatedAt: '2026-10-02T01:00:00.000Z', ...extra })
const directory = { records: [{ userId: 'user-2', displayName: '处理人甲', organizationId: 'org-1' }], total: 75, page: 1, size: 50 }

function context(epoch = 1, routePath = '/micro/work-orders', extra = {}) {
  const actions = ['read', 'create', 'update', 'delete', 'export']
  return { contractVersion: '2.0', appCode: 'work-orders', basePath: '/micro/work-orders', routePath,
    contextVersion: 'fixture-context-' + epoch, contextEpoch: epoch, token: 'fixture-app-token-' + epoch,
    expiresAt: new Date(Date.now() + 60_000).toISOString(), tokenExpiresAt: new Date(Date.now() + 60_000).toISOString(),
    signal: new AbortController().signal, user: { id: 'user-1', displayName: '创建人' },
    tenant: { id: 'tenant-1', name: '测试租户' }, organization: { id: 'org-1', name: '测试组织' }, theme: 'light',
    permissions: actions.map((action) => 'ticket:' + action),
    dataGrants: actions.map((action) => ({ resource: 'ticket', action, scope: 'SELF', subjectUserId: 'user-1', tenantId: 'tenant-1', organizationIds: ['org-1'] })),
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
  const browser = { Vue: vue, __WORK_ORDERS_CONFIG__: { version: 'unit-fixture', allowedHostOrigins: ['https://host.test'], apiBase: '/work-orders-api' },
    DanteMicroIframe: { connect(options) { lifecycle = options.lifecycle; return { downloadCsv: async () => {} } } } }
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
  vm.runInNewContext(appCode, { window: browser, document: { querySelector: (selector) => selector === '#app' ? container : { focus() {} },
    documentElement: { dataset: {} } }, fetch, AbortController, DOMException, Headers, TextEncoder, TextDecoder, URLSearchParams,
    crypto: webcrypto, console }, { filename: 'actual-work-orders-unit-bundle.js' })
  assert.ok(lifecycle, 'actual application registers its SDK lifecycle')
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

test('actual app unit: HTTP 401 invalidates the app and clears a cached ticket and open delete confirmation', async (t) => {
  const app = harness(); t.after(() => app.unmount())
  await app.mount(); await app.click('open-ticket'); await app.click('delete-ticket')
  app.enqueue('DELETE', '/tickets/ticket-a', deny(401))
  await app.click('confirm-delete')
  assert.equal(app.node('work-orders-app').props['data-context-active'], 'false')
  assert.equal(app.nodes('ticket-detail').length, 0)
  assert.equal(app.nodes('delete-confirmation').length, 0)
  assert.equal(app.state.rows.length, 0); assert.equal(app.state.detail, null); assert.equal(app.state.deleteTarget, null)
  assert.doesNotMatch(app.text(), /服务器返回的工单甲/)
  assert.deepEqual(app.failures, [])
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
  app.enqueue('PATCH', '/tickets/ticket-a', reply(409, { code: 'VERSION_CONFLICT', message: '版本冲突' }))
  app.enqueue('GET', '/tickets/ticket-a', deny(404))
  await app.submit()
  assert.equal(app.state.detail, null)
  assert.equal(app.state.rows.some((row) => row.id === 'ticket-a'), false)
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
  app.enqueue('PATCH', '/tickets/ticket-a', reply(409, { message: '版本冲突' }))
  app.enqueue('GET', '/tickets/ticket-a', reply(200, ticket('ticket-a', { version: 2, title: '服务端较新标题' })))
  await app.submit()
  assert.equal(!!app.node('use-latest-version').props.disabled, false)
  app.enqueue('GET', '/tickets/ticket-a', pending.promise)
  await app.update(context(2, '/micro/work-orders/tickets/ticket-a'))
  assert.equal(app.state.editor.latest, null)
  assert.equal(!!app.node('use-latest-version').props.disabled, true)
  assert.equal(app.node('ticket-title').props.value, '保留的输入')
  pending.resolve(deny(404)); await settle()
  assert.equal(app.state.detail, null); assert.equal(app.state.editor.latest, null)
  assert.equal(!!app.node('use-latest-version').props.disabled, true)
  assert.deepEqual(app.failures, [])
})

test('actual app unit: a late detail denial cannot clear a different ticket opened in the same context', async (t) => {
  const app = harness([ticket(), ticket('ticket-b')]), pending = deferred(); t.after(() => app.unmount())
  await app.mount(); await app.click('open-ticket'); await app.click('edit-ticket')
  app.enqueue('PATCH', '/tickets/ticket-a', reply(409, { message: '版本冲突' }))
  app.enqueue('GET', '/tickets/ticket-a', pending.promise)
  await app.submit()
  const cancel = [...walk(app.node('ticket-editor'))].find((node) => node.type === 'button' && textOf(node) === '取消')
  assert.ok(cancel); cancel.props.onClick(); await settle()
  await app.click('back-to-list')
  const secondRow = app.nodes('ticket-row').find((row) => row.props['data-ticket-id'] === 'ticket-b')
  const openSecond = [...walk(secondRow)].find((node) => node.props['data-testid'] === 'open-ticket')
  openSecond.props.onClick(); await settle()
  pending.resolve(deny(404)); await settle()
  assert.equal(app.state.detail.id, 'ticket-b')
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
  app.enqueue('PATCH', '/tickets/ticket-a', reply(200, ticket('ticket-a', { title: '目录被拒绝时保留标题', version: 2 })))
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
    const method = kind === 'create' ? 'POST' : 'PATCH', path = kind === 'create' ? '/tickets' : '/tickets/ticket-a'
    app.enqueue(method, path, deny(403))
    await app.submit()
    assert.equal(app.node('ticket-title').props.value, '403 后仍在的 ' + kind + ' 草稿')
    assert.equal(!!app.node('save-ticket').props.disabled, true)
    const count = app.requests.filter((request) => request.method === method).length
    await app.submit()
    assert.equal(app.requests.filter((request) => request.method === method).length, count, 'actual submit handler blocks a second denied write')
    await app.update(context(2, '/micro/work-orders/tickets/' + (kind === 'create' ? 'new' : 'ticket-a')))
    assert.equal(app.node('ticket-title').props.value, '403 后仍在的 ' + kind + ' 草稿')
    assert.equal(!!app.node('save-ticket').props.disabled, false)
    app.enqueue(method, path, reply(kind === 'create' ? 201 : 200, ticket('ticket-a', { title: '403 后仍在的 ' + kind + ' 草稿', version: 2 })))
    await app.submit()
    assert.equal(app.nodes('ticket-editor').length, 0)
    assert.equal(app.state.detail.title, '403 后仍在的 ' + kind + ' 草稿')
    assert.deepEqual(app.failures, [])
  })
}
