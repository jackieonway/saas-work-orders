import { WorkOrdersClient, BusinessError } from './client.mjs'
import { APP_CODE, STATUSES, PRIORITIES, SCOPE_LABELS, sameIdentity, canTicket as hasTicketGrant, canAssign, canRemind as hasReminderGrant, visibleScope, routeView, ticketDraft, validateDraft, statusChoices } from './domain.mjs'

const { createApp, h, reactive, nextTick } = window.Vue
const config = window.__WORK_ORDERS_CONFIG__
const state = reactive({ active: false, revision: 0, organization: '', tenant: '', person: '', mode: 'list',
  rows: [], total: 0, page: 1, size: 20, status: 'ALL', priority: 'ALL', detail: null, editor: null,
  assignees: [], assigneeQuery: '', assigneePage: 1, assigneeTotal: 0, assigneeError: '', deleteTarget: null, error: '', notice: '', reminders: new Map(), deniedActions: new Set(),
  busy: { list: false, detail: false, save: false, delete: false, export: false, remind: false, assignees: false } })
let vueApp, lastContext, bridge, detailId = null, listSequence = 0, detailSequence = 0, assigneeSequence = 0
let listController, detailController, assigneeController
const client = new WorkOrdersClient({ apiBase: config?.apiBase, onInvalidate: () => {
  state.active = false
  state.rows = []; state.total = 0; state.detail = null; state.error = ''; state.notice = ''
  state.assignees = []; state.assigneeTotal = 0; state.assigneeError = ''; state.deleteTarget = null
  if (state.editor) state.editor.latest = null
  for (const key of Object.keys(state.busy)) state.busy[key] = false
  listController?.abort(); detailController?.abort(); assigneeController?.abort()
} })
const current = () => { state.revision; return client.current() }
function canTicket(props, action, ticket) { return !state.deniedActions.has('ticket:' + action) && hasTicketGrant(props, action, ticket) }
function canRemind(props, ticket) { return !state.deniedActions.has('notification:publish') && canTicket(props, 'read', ticket) && hasReminderGrant(props, ticket) }
const humanError = (error) => error instanceof BusinessError ? error.message : '操作暂未完成，请稍后重试'
const ignore = (error, snapshot) => error?.name === 'AbortError' || !client.isCurrent(snapshot)
function success(message) { state.notice = message; current()?.onNotify?.(message, 'success') }
function clearMessages() { state.error = ''; state.notice = '' }
function navigate(path) { current()?.onNavigate?.('/micro/' + APP_CODE + path) }

async function loadList() {
  const props = current()
  if (!props || !canTicket(props, 'read')) return
  listController?.abort(); listController = new AbortController()
  const sequence = ++listSequence, snapshot = client.capture()
  state.busy.list = true; state.error = ''
  try {
    const result = await client.list({ page: state.page, size: state.size, status: state.status, priority: state.priority }, listController.signal)
    if (!client.isCurrent(snapshot) || sequence !== listSequence) return
    state.rows = result.records; state.total = result.total
  } catch (error) {
    if (!ignore(error, snapshot) && sequence === listSequence) {
      if (error.status === 403) {
        state.rows = []; state.total = 0; state.detail = null; state.deleteTarget = null
        if (state.editor) state.editor.latest = null
        state.deniedActions.add('ticket:read')
      }
      state.error = humanError(error)
    }
  }
  finally { if (client.isCurrent(snapshot) && sequence === listSequence) state.busy.list = false }
}
async function loadDetail(id = detailId) {
  if (!current() || !id) return
  detailController?.abort(); detailController = new AbortController()
  const sequence = ++detailSequence, snapshot = client.capture()
  if (state.editor?.ticket?.id === id) state.editor.latest = null
  state.busy.detail = true; state.error = ''
  try {
    const result = await client.detail(id, detailController.signal)
    if (!client.isCurrent(snapshot) || sequence !== detailSequence || detailId !== id) return
    state.detail = result
    if (state.editor?.ticket?.id === id && state.editor.conflict) state.editor.latest = result
    return result
  } catch (error) {
    if (!ignore(error, snapshot) && sequence === detailSequence && detailId === id) {
      if (error.status === 403 || error.status === 404) {
        state.detail = null
        const previousLength = state.rows.length
        state.rows = state.rows.filter((ticket) => ticket.id !== id)
        state.total = Math.max(0, state.total - (previousLength - state.rows.length))
        if (state.deleteTarget?.id === id) state.deleteTarget = null
        if (state.editor?.ticket?.id === id) state.editor.latest = null
        const intent = state.reminders.get(id)
        if (intent?.conflict) intent.recipientConfirmed = false
      }
      state.error = humanError(error)
    }
  }
  finally { if (client.isCurrent(snapshot) && sequence === detailSequence) state.busy.detail = false }
}
function openDetail(ticket) {
  if (!current() || !canTicket(current(), 'read', ticket)) return
  detailId = ticket.id; state.mode = 'detail'; state.detail = ticket; clearMessages()
  navigate('/tickets/' + ticket.id)
}
function backToList() { state.mode = 'list'; detailId = null; state.detail = null; state.editor = null; clearMessages(); navigate(''); void loadList() }
async function loadAssignees() {
  if (!state.editor || state.editor.assignmentDenied || !canTicket(current(), state.editor.kind === 'create' ? 'create' : 'update', state.editor.ticket) || !canAssign(current(), state.editor.ticket)) return
  assigneeController?.abort(); assigneeController = new AbortController()
  const sequence = ++assigneeSequence, snapshot = client.capture(), editor = state.editor
  state.busy.assignees = true; state.assigneeError = ''
  try {
    const result = await client.assignees(editor.kind === 'create' ? 'create' : 'update',
      editor.ticket?.organizationId || snapshot.props.organization.id, assigneeController.signal,
      { q: state.assigneeQuery.trim(), page: state.assigneePage, size: 50 })
    if (client.isCurrent(snapshot) && sequence === assigneeSequence && state.editor === editor) {
      state.assignees = result.records; state.assigneeTotal = result.total
    }
  } catch (error) {
    if (!ignore(error, snapshot) && sequence === assigneeSequence && state.editor === editor) {
      if (error.status === 403) { state.assignees = []; state.assigneeTotal = 0; editor.assignmentDenied = true }
      state.assigneeError = humanError(error)
    }
  }
  finally { if (client.isCurrent(snapshot) && sequence === assigneeSequence) state.busy.assignees = false }
}
function openEditor(ticket = null) {
  const props = current(), kind = ticket ? 'edit' : 'create'
  if (!props || !canTicket(props, ticket ? 'update' : 'create', ticket)) return
  state.editor = { kind, ticket, draft: ticketDraft(ticket), assigneeLabel: ticket?.assigneeDisplayName || '', assignmentDenied: false, conflict: false, latest: null }
  state.assignees = []; state.assigneeQuery = ''; state.assigneePage = 1; state.assigneeTotal = 0; state.assigneeError = ''; clearMessages()
  if (!ticket) navigate('/tickets/new')
  void loadAssignees()
  void nextTick(() => document.querySelector('[data-testid="ticket-title"]')?.focus())
}
function closeEditor() {
  if (state.busy.save) return
  const wasCreate = state.editor?.kind === 'create'
  state.editor = null; assigneeController?.abort(); clearMessages()
  if (wasCreate) { state.mode = 'list'; navigate(''); void loadList() }
}
async function saveTicket() {
  const editor = state.editor, props = current()
  if (!editor || !props || state.busy.save || editor.conflict || !canTicket(props, editor.kind === 'create' ? 'create' : 'update', editor.ticket)) return
  const validation = validateDraft(editor.draft)
  if (validation) { state.error = validation; return }
  const snapshot = client.capture(), draft = editor.draft
  const body = { title: draft.title.trim(), description: draft.description, priority: draft.priority }
  if (editor.kind === 'edit') { body.version = draft.version; body.status = draft.status }
  if (!editor.assignmentDenied && canAssign(props, editor.ticket)) body.assigneeUserId = draft.assigneeUserId || null
  state.busy.save = true; clearMessages()
  try {
    const ticket = await (editor.kind === 'create' ? client.create(body) : client.updateTicket(editor.ticket.id, body))
    if (!client.isCurrent(snapshot)) return
    state.editor = null; state.mode = 'detail'; detailId = ticket.id; state.detail = ticket
    success(editor.kind === 'create' ? `工单 ${ticket.number} 已创建` : '工单更改已保存')
    if (canTicket(current(), 'read', ticket)) navigate('/tickets/' + ticket.id)
    else backToList()
  } catch (error) {
    if (ignore(error, snapshot)) return
    if (error.status === 403) state.deniedActions.add('ticket:' + (editor.kind === 'create' ? 'create' : 'update'))
    state.error = error.status === 409 ? '此工单已有新的更改。请查看最新内容，再决定如何保存你的修改。' : humanError(error)
    if (error.status === 409 && editor.kind === 'edit') { editor.conflict = true; void loadDetail(editor.ticket.id) }
  } finally { if (client.isCurrent(snapshot)) state.busy.save = false }
}
function useLatestVersion() {
  const editor = state.editor
  if (!editor?.latest || !canTicket(current(), 'update', editor.latest)) return
  if (!Object.hasOwn(statusChoices(editor.latest.status), editor.draft.status)) editor.draft.status = editor.latest.status
  editor.ticket = editor.latest; editor.draft.version = editor.latest.version; editor.conflict = false; editor.latest = null
  state.error = ''; void loadAssignees()
}
async function changeStatus(status) {
  const ticket = state.detail, props = current()
  if (!ticket || !props || !canTicket(props, 'update', ticket) || state.busy.save) return
  const snapshot = client.capture(); state.busy.save = true; clearMessages()
  try {
    const result = await client.updateTicket(ticket.id, { version: ticket.version, status })
    if (client.isCurrent(snapshot)) { state.detail = result; success(status === 'RESOLVED' ? '工单已标记为解决' : status === 'IN_PROGRESS' && ticket.status === 'RESOLVED' ? '工单已重新打开处理' : '工单已开始处理') }
  } catch (error) {
    if (!ignore(error, snapshot)) {
      if (error.status === 403) state.deniedActions.add('ticket:update')
      state.error = error.status === 409 ? '工单已有新的更改，请刷新后重试。' : humanError(error)
      if (error.status === 409) void loadDetail()
    }
  } finally { if (client.isCurrent(snapshot)) state.busy.save = false }
}
async function deleteTicket() {
  const ticket = state.deleteTarget, props = current()
  if (!ticket || !props || !canTicket(props, 'delete', ticket) || state.busy.delete) return
  const snapshot = client.capture(); state.busy.delete = true; clearMessages()
  try {
    await client.remove(ticket.id, ticket.version)
    if (!client.isCurrent(snapshot)) return
    state.deleteTarget = null; backToList(); success(`工单 ${ticket.number} 已移出列表`)
  } catch (error) {
    if (!ignore(error, snapshot)) {
      if (error.status === 403) state.deniedActions.add('ticket:delete')
      state.error = error.status === 409 ? '工单已有新的更改，请关闭确认框并刷新后再删除。' : humanError(error)
    }
  } finally { if (client.isCurrent(snapshot)) state.busy.delete = false }
}
async function exportTickets() {
  if (!current() || !canTicket(current(), 'export') || state.busy.export) return
  const snapshot = client.capture(); state.busy.export = true; clearMessages()
  try {
    const csv = await client.exportCsv({ status: state.status, priority: state.priority })
    client.check(snapshot)
    await bridge.downloadCsv({ resource: 'ticket', filename: '工单.csv', content: csv })
    client.check(snapshot); success('工单 CSV 已开始下载')
  } catch (error) {
    if (!ignore(error, snapshot)) { if (error.status === 403) state.deniedActions.add('ticket:export'); state.error = humanError(error) }
  }
  finally { if (client.isCurrent(snapshot)) state.busy.export = false }
}
async function remindTicket(newEvent = false) {
  const ticket = state.detail
  if (!ticket || !canRemind(current(), ticket) || state.busy.remind) return
  if (!state.reminders.has(ticket.id) || newEvent) state.reminders.set(ticket.id, { ticketId: ticket.id,
    key: 'ticket:' + ticket.id + ':manual:' + crypto.randomUUID(), conflict: false })
  const intent = state.reminders.get(ticket.id), snapshot = client.capture()
  state.busy.remind = true; clearMessages()
  try {
    const result = await client.remind(ticket.id, intent.key)
    if (!client.isCurrent(snapshot)) return
    state.reminders.delete(ticket.id)
    success(result.duplicate ? '此前的工单提醒已确认，无需重复发送' : '工单提醒已发送')
  } catch (error) {
    if (ignore(error, snapshot)) return
    if (error.status === 409) {
      intent.conflict = true; intent.recipientConfirmed = false
      await refreshReminderTarget(ticket.id)
      if (client.isCurrent(snapshot)) state.error = '提醒对象已变化，旧事件不能重发。请确认当前显示的处理人后，发送一条新提醒。'
    }
    else if (error.status < 500) {
      if (error.status === 403) state.deniedActions.add('notification:publish')
      state.reminders.delete(ticket.id); state.error = humanError(error)
    }
    else state.error = '提醒结果尚未确认。请使用“重试提醒”，系统会沿用同一事件，避免重复投递。'
  } finally { if (client.isCurrent(snapshot)) state.busy.remind = false }
}
async function refreshReminderTarget(id = detailId) {
  const intent = state.reminders.get(id)
  if (!intent?.conflict || !current()) return
  const snapshot = client.capture(), latest = await loadDetail(id)
  if (latest && client.isCurrent(snapshot) && state.reminders.get(id) === intent) intent.recipientConfirmed = true
}

function acceptProps(props) {
  const same = sameIdentity(lastContext, props)
  state.deniedActions.clear()
  if (!same) {
    state.editor = null; state.deleteTarget = null; state.reminders.clear()
    state.page = 1; state.status = 'ALL'; state.priority = 'ALL'; state.notice = ''
  }
  lastContext = props; client.update(props)
  if (state.editor) state.editor.assignmentDenied = false
  state.active = true; state.revision++; state.organization = props.organization.name; state.tenant = props.tenant.name; state.person = props.user.displayName
  document.documentElement.dataset.theme = props.theme
  const route = routeView(props.routePath)
  state.mode = route.kind === 'detail' ? 'detail' : 'list'; detailId = route.kind === 'detail' ? route.id : null
  if (state.editor && !canTicket(props, state.editor.kind === 'create' ? 'create' : 'update', state.editor.ticket)) state.editor = null
  if (route.kind === 'create' && !state.editor && canTicket(props, 'create')) openEditor()
  if (detailId) void loadDetail(detailId)
  else void loadList()
  if (state.editor) void loadAssignees()
}

const icons = { check: 'M5 12l4 4L19 6', plus: 'M12 5v14M5 12h14', arrow: 'M5 12h14M13 6l6 6-6 6', back: 'M19 12H5M11 6l-6 6 6 6',
  refresh: 'M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-1l2 2M18 17a7 7 0 0 1-12 1l-2-2',
  download: 'M12 3v12M7 10l5 5 5-5M4 17v4h16v-4', close: 'M6 6l12 12M18 6L6 18', edit: 'M15 4l5 5-12 12H3v-5L15 4Z',
  bell: 'M6 16V9a6 6 0 0 1 12 0v7l2 2H4l2-2M10 21h4', trash: 'M3 6h18M8 6V3h8v3M6 6l1 15h10l1-15' }
const icon = (name) => h('svg', { width: 16, height: 16, viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', 'stroke-width': 1.7, 'stroke-linecap': 'round', 'stroke-linejoin': 'round', 'aria-hidden': true }, [h('path', { d: icons[name] || icons.arrow })])
const button = (text, action, { kind = 'secondary', test, disabled = false, symbol } = {}) => h('button', {
  type: 'button', class: ['button', kind], onClick: action, disabled: disabled || !state.active,
  ...(test ? { 'data-testid': test } : {}) }, [symbol ? icon(symbol) : null, text])
const tag = (value, labels, kind) => h('span', { class: ['tag', kind, value.toLowerCase()] }, labels[value] || value)
const formatTime = (value) => value ? new Intl.DateTimeFormat('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }).format(new Date(value)) : '—'
function field(label, control, hint) { return h('label', { class: 'field' }, [h('span', { class: 'field-label' }, label), control, hint ? h('small', hint) : null]) }
function select(value, values, onChange, test, disabled = false) {
  return h('select', { value, onChange: (event) => onChange(event.target.value), disabled, 'data-testid': test },
    Object.entries(values).map(([key, label]) => h('option', { value: key }, label)))
}
function trapDialog(event) {
  if (event.key === 'Escape' && !state.busy.save && !state.busy.delete) { if (state.deleteTarget) state.deleteTarget = null; else closeEditor(); return }
  if (event.key !== 'Tab') return
  const targets = [...event.currentTarget.querySelectorAll('button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled)')]
  if (!targets.length) return
  const first = targets[0], last = targets.at(-1)
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus() }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus() }
}
function renderList(props) {
  const allowed = canTicket(props, 'read')
  return h('section', { class: 'work-surface', 'data-testid': 'ticket-list' }, [
    h('div', { class: 'surface-toolbar' }, [h('div', { class: 'filters' }, [
      field('状态', select(state.status, { ALL: '全部状态', ...STATUSES }, (value) => { state.status = value; state.page = 1; void loadList() }, 'filter-status')),
      field('优先级', select(state.priority, { ALL: '全部优先级', ...PRIORITIES }, (value) => { state.priority = value; state.page = 1; void loadList() }, 'filter-priority')),
    ]), h('div', { class: 'toolbar-right' }, [h('span', { class: 'result-count' }, `${state.total} 条工单`),
      button('刷新', loadList, { symbol: 'refresh', test: 'refresh-tickets', disabled: state.busy.list || !allowed })])]),
    !allowed ? h('div', { class: 'empty-state' }, [h('h2', '暂未获得工单查看授权'), h('p', '请联系组织管理员开通查看操作及对应的数据范围。')]) :
      state.busy.list ? h('div', { class: 'loading-state', role: 'status' }, [h('span', { class: 'spinner' }), '正在读取工单…']) :
        !state.rows.length ? h('div', { class: 'empty-state' }, [h('div', { class: 'empty-mark' }, icon('check')), h('h2', '当前范围内没有匹配的工单'), h('p', '调整筛选条件，或创建一张新的工单。')]) :
          h('div', { class: 'table-scroll' }, h('table', { class: 'ticket-table' }, [
            h('thead', h('tr', ['工单', '状态', '优先级', '所属组织', '处理人', '更新于'].map((label) => h('th', label)))),
            h('tbody', state.rows.map((ticket) => h('tr', { 'data-testid': 'ticket-row', 'data-ticket-id': ticket.id, 'data-version': ticket.version }, [
              h('td', h('button', { class: 'ticket-link', onClick: () => openDetail(ticket), 'data-testid': 'open-ticket' }, [
                h('span', { class: 'ticket-number', 'data-testid': 'ticket-number' }, ticket.number), h('strong', ticket.title),
                h('span', { class: 'creator' }, `创建人 ${ticket.ownerDisplayName}${ticket.ownerUserId === props.user.id ? ' · 我' : ''}`)])),
              h('td', { 'data-testid': 'ticket-status' }, tag(ticket.status, STATUSES, 'status')),
              h('td', { 'data-testid': 'ticket-priority' }, tag(ticket.priority, PRIORITIES, 'priority')),
              h('td', ticket.organizationName), h('td', ticket.assigneeDisplayName || h('span', { class: 'muted' }, '未分配')),
              h('td', { class: 'time-cell' }, formatTime(ticket.updatedAt)),
            ]))),
          ])),
    h('footer', { class: 'pagination' }, [h('span', `第 ${state.page} 页 · 每页 ${state.size} 条`), h('div', [
      button('上一页', () => { state.page--; void loadList() }, { test: 'previous-page', disabled: state.page <= 1 || state.busy.list }),
      button('下一页', () => { state.page++; void loadList() }, { test: 'next-page', disabled: state.page * state.size >= state.total || state.busy.list }),
    ])]),
  ])
}
function renderDetail(props) {
  const ticket = state.detail
  const reminder = ticket ? state.reminders.get(ticket.id) : null
  return h('section', { class: 'detail-layout', 'data-testid': 'ticket-detail' }, [
    button('返回工单列表', backToList, { symbol: 'back', test: 'back-to-list', kind: 'text-button' }),
    state.busy.detail ? h('div', { class: 'loading-state', role: 'status' }, '正在读取工单详情…') : ticket ? h('div', { class: 'detail-card', 'data-ticket-id': ticket.id, 'data-version': ticket.version }, [
      h('div', { class: 'detail-heading' }, [h('div', [h('span', { class: 'ticket-number', 'data-testid': 'detail-number' }, ticket.number), h('h2', ticket.title)]), tag(ticket.status, STATUSES, 'status')]),
      h('div', { class: 'detail-actions' }, [
        canTicket(props, 'update', ticket) ? button('编辑工单', () => openEditor(ticket), { symbol: 'edit', test: 'edit-ticket' }) : null,
        canTicket(props, 'update', ticket) ? button(ticket.status === 'RESOLVED' ? '重新处理' : ticket.status === 'OPEN' ? '开始处理' : '标记解决',
          () => changeStatus(ticket.status === 'IN_PROGRESS' ? 'RESOLVED' : 'IN_PROGRESS'), { kind: 'primary', test: 'change-ticket-status', disabled: state.busy.save }) : null,
        canRemind(props, ticket) ? button(reminder?.conflict ? '作为新提醒发送' : reminder ? '重试提醒' : '发送提醒',
          () => remindTicket(!!reminder?.conflict), { symbol: 'bell', test: 'remind-ticket',
            disabled: state.busy.remind || state.busy.detail || (reminder?.conflict && !reminder.recipientConfirmed) }) : null,
        reminder?.conflict && !reminder.recipientConfirmed ? button('刷新处理人', () => refreshReminderTarget(ticket.id),
          { symbol: 'refresh', test: 'refresh-reminder-target', disabled: state.busy.remind || state.busy.detail }) : null,
        canTicket(props, 'delete', ticket) ? button('删除', () => { state.deleteTarget = ticket; clearMessages() }, { symbol: 'trash', kind: 'danger-subtle', test: 'delete-ticket' }) : null,
      ]),
      h('dl', { class: 'detail-meta' }, [ ['优先级', PRIORITIES[ticket.priority]], ['所属组织', ticket.organizationName],
        ['创建人', ticket.ownerDisplayName], ['处理人', ticket.assigneeDisplayName || '未分配'],
        ['创建时间', formatTime(ticket.createdAt)], ['最近更新', formatTime(ticket.updatedAt)],
      ].map(([label, value]) => h('div', [h('dt', label), h('dd', value)]))),
      h('section', { class: 'description' }, [h('h3', '工单描述'), h('p', ticket.description || '尚未填写描述')]),
      h('div', { class: 'detail-footnote' }, [`修订 ${ticket.version}`, h('span', '提醒独立发送，不会重复保存工单。')]),
    ]) : h('div', { class: 'empty-state' }, [h('h2', '无法显示此工单'), h('p', '工单可能已删除，或不在当前可见范围内。')]),
  ])
}
function renderEditor(props) {
  const editor = state.editor
  if (!editor) return null
  const draft = editor.draft, assignmentAllowed = !editor.assignmentDenied && canTicket(props, editor.kind === 'create' ? 'create' : 'update', editor.ticket) && canAssign(props, editor.ticket)
  const options = { '': '暂不指定', ...Object.fromEntries(state.assignees.map((user) => [user.userId, user.displayName])) }
  if (draft.assigneeUserId && !options[draft.assigneeUserId]) options[draft.assigneeUserId] = editor.assigneeLabel || editor.ticket?.assigneeDisplayName || '已选择的处理人'
  return h('div', { class: 'modal-scrim' }, h('section', { class: 'modal', role: 'dialog', 'aria-modal': true,
    'aria-labelledby': 'ticket-editor-heading', 'data-testid': 'ticket-editor', onKeydown: trapDialog }, [
    h('header', { class: 'modal-header' }, [h('div', [h('p', { class: 'eyebrow' }, editor.kind === 'create' ? '新的工作事项' : editor.ticket.number),
      h('h2', { id: 'ticket-editor-heading' }, editor.kind === 'create' ? '新建工单' : '编辑工单')]),
      button('关闭', closeEditor, { symbol: 'close', kind: 'text-button', disabled: state.busy.save })]),
    h('form', { class: 'ticket-form', onSubmit: (event) => { event.preventDefault(); void saveTicket() } }, [
      h('div', { class: 'form-fields', 'data-testid': 'ticket-editor-fields' }, [
      editor.conflict ? h('div', { class: 'conflict-box', role: 'alert', 'data-testid': 'version-conflict' }, [
        h('strong', '检测到新的更改'), h('p', `最新标题：${editor.latest?.title || '正在读取…'}`),
        h('p', '你的输入已保留。确认最新内容后，可使用新修订继续提交自己的修改。'),
        button('使用最新版本继续编辑', useLatestVersion, { test: 'use-latest-version', disabled: !editor.latest }),
      ]) : null,
      field('标题', h('input', { value: draft.title, onInput: (event) => { draft.title = event.target.value },
        maxlength: 200, required: true, placeholder: '清楚描述需要完成的事项', 'data-testid': 'ticket-title', disabled: state.busy.save })),
      h('div', { class: 'form-grid' }, [field('优先级', select(draft.priority, PRIORITIES, (value) => { draft.priority = value }, 'ticket-priority-input', state.busy.save)),
        editor.kind === 'edit' ? field('状态', select(draft.status, statusChoices(editor.ticket.status), (value) => { draft.status = value }, 'ticket-status-input', state.busy.save)) : field('初始状态', h('div', { class: 'read-only-field' }, '待处理'))]),
      field('处理人', select(draft.assigneeUserId, options, (value) => { draft.assigneeUserId = value; editor.assigneeLabel = options[value] }, 'ticket-assignee',
        state.busy.save || !assignmentAllowed), assignmentAllowed ? '可选；候选来自工单所属组织的有效直接成员。' : '在工单所属组织或具备对应组织范围时，才可调整处理人。'),
      assignmentAllowed ? h('div', { class: 'assignee-directory' }, [
        h('div', { class: 'directory-search' }, [h('input', { value: state.assigneeQuery, maxlength: 100, placeholder: '按姓名搜索处理人',
          'aria-label': '搜索处理人姓名', 'data-testid': 'assignee-search', disabled: state.busy.save,
          onInput: (event) => { state.assigneeQuery = event.target.value },
          onKeydown: (event) => { if (event.key === 'Enter') { event.preventDefault(); state.assigneePage = 1; void loadAssignees() } } }),
        button('搜索', () => { state.assigneePage = 1; void loadAssignees() }, { test: 'search-assignees', disabled: state.busy.save || state.busy.assignees })]),
        h('div', { class: 'directory-pagination' }, [h('span', state.busy.assignees ? '正在读取候选成员…' : `${state.assigneeTotal} 位候选 · 第 ${state.assigneePage} 页`), h('div', [
          button('上一页', () => { state.assigneePage--; void loadAssignees() }, { test: 'previous-assignees', disabled: state.busy.save || state.busy.assignees || state.assigneePage <= 1 }),
          button('下一页', () => { state.assigneePage++; void loadAssignees() }, { test: 'next-assignees', disabled: state.busy.save || state.busy.assignees || state.assigneePage * 50 >= state.assigneeTotal }),
        ])]),
      ]) : null,
      state.assigneeError ? h('p', { class: 'inline-warning' }, state.assigneeError) : null,
      field('描述', h('textarea', { value: draft.description, onInput: (event) => { draft.description = event.target.value }, rows: 5,
        maxlength: 8192, placeholder: '补充背景、预期结果与必要说明', 'data-testid': 'ticket-description', disabled: state.busy.save })),
      editor.kind === 'create' ? h('p', { class: 'form-note' }, `所属组织为${state.organization}，创建人为${state.person}，由服务端确认。`) : null,
      state.error ? h('p', { class: 'inline-error', role: 'alert' }, state.error) : null,
      ]),
      h('footer', { class: 'modal-footer' }, [button('取消', closeEditor, { disabled: state.busy.save }), h('button', {
        type: 'submit', class: 'button primary', 'data-testid': 'save-ticket', disabled: !state.active || state.busy.save || editor.conflict || !canTicket(props, editor.kind === 'create' ? 'create' : 'update', editor.ticket) },
      state.busy.save ? '正在保存…' : editor.kind === 'create' ? '创建工单' : '保存更改')]),
    ]),
  ]))
}
function renderDelete() {
  if (!state.deleteTarget) return null
  const ticket = state.deleteTarget
  return h('div', { class: 'modal-scrim' }, h('section', { class: 'modal small-modal', role: 'alertdialog', 'aria-modal': true,
    'aria-labelledby': 'delete-title', 'data-testid': 'delete-confirmation', onKeydown: trapDialog }, [
    h('h2', { id: 'delete-title' }, '将工单移出列表？'), h('p', { class: 'delete-caption' }, `${ticket.number} · ${ticket.title}`),
    h('p', '删除后不再显示在列表、详情与导出中；历史记录仍保留。'),
    state.error ? h('p', { class: 'inline-error', role: 'alert' }, state.error) : null,
    h('footer', { class: 'modal-footer' }, [button('取消', () => { state.deleteTarget = null }, { disabled: state.busy.delete }),
      button(state.busy.delete ? '正在删除…' : '确认删除', deleteTicket, { kind: 'danger', test: 'confirm-delete', disabled: state.busy.delete || !canTicket(current(), 'delete', ticket) })]),
  ]))
}
function renderApp() {
  const props = current(), scope = props ? visibleScope(props) : null
  return h('main', { class: 'work-orders-app', 'data-testid': 'work-orders-app', 'data-version': config.version,
    'data-context-active': String(state.active), 'data-org-id': props?.organization.id || '' }, [
    h('header', { class: 'app-header' }, [h('div', { class: 'app-heading' }, [h('span', { class: 'app-mark' }, 'W'),
      h('div', [h('p', { class: 'eyebrow' }, '工作跟进 · 工单管理'), h('h1', '工单中心'), h('p', { class: 'heading-caption' }, '把待办推进成结果，每一步都有清晰的归属。')])]),
      h('div', { class: 'context-summary' }, [h('strong', state.organization || '正在连接组织'), h('span', state.tenant)])]),
    h('div', { class: 'app-command-bar' }, [h('div', { class: 'scope-summary' }, [h('span', { class: 'scope-dot' }),
      h('span', scope ? `可见范围 · ${SCOPE_LABELS[scope]}` : '当前工单空间'), scope === 'SELF' ? h('small', '按创建人归属，处理人不扩大范围') : null]),
      h('div', { class: 'command-actions' }, [canTicket(props, 'export') ? button(state.busy.export ? '正在导出…' : '导出 CSV', exportTickets,
        { symbol: 'download', test: 'export-tickets', disabled: state.busy.export }) : null,
      canTicket(props, 'create') ? button('新建工单', () => openEditor(), { symbol: 'plus', kind: 'primary', test: 'create-ticket' }) : null])]),
    !state.active ? h('div', { class: 'context-pending', role: 'status' }, [h('span', { class: 'spinner' }), '正在同步当前组织的访问权限…']) :
      h('div', { class: 'page-content' }, [state.error && !state.editor && !state.deleteTarget ? h('div', { class: 'error-banner', role: 'alert' }, state.error) : null,
        state.notice ? h('div', { class: 'success-banner', role: 'status' }, state.notice) : null,
        state.mode === 'detail' ? renderDetail(props) : renderList(props)]),
    h('footer', { class: 'app-footer' }, [h('span', '清晰记录 · 有序协作'), h('span', { 'data-testid': 'app-version', 'data-version': config.version }, `工单中心 ${config.version}`)]),
    state.active ? renderEditor(props) : null, state.active ? renderDelete() : null,
  ])
}
if (!config?.allowedHostOrigins?.length || !window.MicroIframe?.connect) {
  document.querySelector('#app').textContent = '工单应用尚未配置宿主连接，请联系平台管理员。'
} else {
  bridge = window.MicroIframe.connect({ appCode: APP_CODE, allowedHostOrigins: config.allowedHostOrigins,
    container: document.querySelector('#app'), lifecycle: {
      mount(container, props) { vueApp = createApp({ render: renderApp }); vueApp.mount(container); acceptProps(props) },
      update(props) { acceptProps(props) },
      unmount() { client.stop(); vueApp?.unmount(); vueApp = null; lastContext = null },
    } })
}
