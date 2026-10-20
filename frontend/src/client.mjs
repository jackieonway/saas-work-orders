import { validContext, validIdempotencyKey, MAX_BYTES, PRIORITIES, STATUSES } from './domain.mjs'
import { SDK_VERSION, MicroSdkError, requestWithContext, authorizationContextSignature, assertSupportedSdkVersion } from '../vendor/micro-sdk.mjs'

export class BusinessError extends Error {
  constructor(status, message, code = '', kind = 'HTTP', requestId) {
    super(message); this.name = 'BusinessError'; this.status = status; this.code = code; this.kind = kind
    if (requestId) this.requestId = requestId
  }
}
const cancelled = () => new DOMException('工单上下文已变更', 'AbortError')
const unknownCodes = new Set(['CREATE_RESULT_UNKNOWN', 'WRITE_RESULT_UNKNOWN', 'REMINDER_PREPARATION_UNKNOWN', 'REMINDER_RESULT_UNKNOWN'])
const uuid = value => typeof value === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value)
const string = (value, maximum) => typeof value === 'string' && !value.includes('\0') && Array.from(value).length <= maximum
const timestamp = value => typeof value === 'string' && value.length <= 64 &&
  /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(value) && Number.isFinite(Date.parse(value))
function requireShape(condition) { if (!condition) throw new TypeError('业务服务返回的数据无法识别') }
function pick(value, fields) { return Object.fromEntries(fields.map(field => [field, value[field]])) }

function ticketResponse(value) {
  requireShape(object(value) && ['id', 'tenantId', 'organizationId', 'ownerUserId'].every(field => uuid(value[field])) &&
    string(value.number, 40) && !!value.number && string(value.title, 200) && !!value.title.trim() &&
    string(value.description, 8192) && string(value.organizationName, 100) && string(value.ownerDisplayName, 100) &&
    Object.hasOwn(PRIORITIES, value.priority) && Object.hasOwn(STATUSES, value.status) &&
    Number.isInteger(value.version) && value.version >= 1 && value.version <= 2147483647 &&
    timestamp(value.createdAt) && timestamp(value.updatedAt) &&
    ((value.assigneeUserId === null && value.assigneeDisplayName === null) ||
      (uuid(value.assigneeUserId) && string(value.assigneeDisplayName, 100))))
  return pick(value, ['id', 'number', 'title', 'description', 'priority', 'status', 'tenantId', 'organizationId',
    'organizationName', 'ownerUserId', 'ownerDisplayName', 'assigneeUserId', 'assigneeDisplayName', 'version', 'createdAt', 'updatedAt'])
}
function ticketFor(id, value) {
  const ticket = ticketResponse(value)
  requireShape(typeof id === 'string' && ticket.id.toLowerCase() === id.toLowerCase())
  return ticket
}
function pageResponse(value, decodeRecord, maximumSize) {
  requireShape(object(value) && Array.isArray(value.records) && Number.isSafeInteger(value.total) && value.total >= 0 &&
    Number.isInteger(value.page) && value.page >= 1 && value.page <= 1000000 &&
    Number.isInteger(value.size) && value.size >= 1 && value.size <= maximumSize &&
    value.records.length <= value.size && value.records.length <= value.total)
  return { records: value.records.map(decodeRecord), total: value.total, page: value.page, size: value.size }
}
function memberResponse(value) {
  requireShape(object(value) && uuid(value.userId) && uuid(value.organizationId) && string(value.displayName, 100))
  return pick(value, ['userId', 'displayName', 'organizationId'])
}
function reminderResponse(value) {
  requireShape(object(value) && uuid(value.publicationId) && value.recipientCount === 1 && value.deliveryCount === 1 &&
    typeof value.duplicate === 'boolean' && timestamp(value.createdAt))
  return pick(value, ['publicationId', 'recipientCount', 'deliveryCount', 'duplicate', 'createdAt'])
}
function deleteResponse(value) { requireShape(object(value) && value.ok === true); return { ok: true } }

// The registered SDK deliberately discards error bodies. Only these bounded
// business codes affect recovery; remote messages never enter the UI or errors.
async function unknownCode(response, signal) {
  if (response?.status !== 503 || !response.body?.getReader) return ''
  const limit = 8192
  if (Number(response.headers?.get('Content-Length')) > limit) { response.body.cancel().catch(() => {}); return '' }
  const reader = response.body.getReader(), decoder = new TextDecoder('utf-8', { fatal: true })
  let length = 0, text = '', rejectAbort
  const aborted = new Promise((_, reject) => { rejectAbort = reject })
  const abort = () => { rejectAbort(cancelled()); reader.cancel().catch(() => {}) }
  signal.addEventListener('abort', abort, { once: true })
  if (signal.aborted) abort()
  try {
    while (true) {
      const { value, done } = await Promise.race([reader.read(), aborted])
      if (done) break
      length += value.byteLength
      if (length > limit) return ''
      text += decoder.decode(value, { stream: true })
    }
    const code = JSON.parse(text + decoder.decode())?.code
    return unknownCodes.has(code) ? code : ''
  } catch { return '' }
  finally {
    signal.removeEventListener('abort', abort)
    reader.cancel().catch(() => {})
    try { reader.releaseLock() } catch {}
  }
}

/** 只发送捕获的应用 token；旧 fetch/json 即使忽略取消，也不能返回到新上下文。 */
export class WorkOrdersClient {
  constructor({ apiBase = '/work-orders-api', fetchImpl = fetch, onInvalidate = () => {} } = {}) {
    this.sdkVersion = SDK_VERSION
    this.apiBase = apiBase.replace(/\/$/, '')
    this.fetchImpl = fetchImpl
    this.onInvalidate = onInvalidate
    this.props = null
    this.generation = 0
    this.controller = null
    this.unbind = null
    this.authorizationKey = null
    this.hostSignal = null
  }
  current() { return validContext(this.props) && !this.controller?.signal.aborted ? this.props : null }
  update(props) {
    try { if (props?.sdkVersion !== undefined) assertSupportedSdkVersion(props.sdkVersion) }
    catch { this.stop(); throw cancelled() }
    if (!validContext(props)) { this.stop(); throw cancelled() }
    const nextKey = authorizationContextSignature(props)
    if (this.current() && nextKey !== null && nextKey === this.authorizationKey && this.hostSignal === props.signal) {
      this.props = props
      return false
    }
    this.stop()
    this.props = props
    this.authorizationKey = nextKey
    const lifetime = new AbortController()
    this.controller = lifetime
    const source = props.signal
    this.hostSignal = source
    const abort = () => { if (this.controller === lifetime) this.stop() }
    source.addEventListener('abort', abort, { once: true })
    this.unbind = () => source.removeEventListener('abort', abort)
    if (source.aborted) abort()
    return true
  }
  stop() {
    this.generation++
    this.controller?.abort()
    this.unbind?.()
    this.unbind = null
    this.props = null
    this.controller = null
    this.authorizationKey = null
    this.hostSignal = null
    this.onInvalidate()
  }
  capture() {
    const props = this.current()
    if (!props) throw cancelled()
    return { props, generation: this.generation, controller: this.controller }
  }
  isCurrent(snapshot) { return snapshot.generation === this.generation && snapshot.controller === this.controller && !!this.current() }
  check(snapshot) { if (!this.isCurrent(snapshot)) throw cancelled() }
  async request(path, { method = 'GET', body, signal, text = false, idempotencyKey, uncertainOutcome, decode, expectedStatus = 200 } = {}) {
    const snapshot = this.capture(), controller = new AbortController()
    const lifetime = this.controller.signal
    const abort = () => controller.abort()
    lifetime.addEventListener('abort', abort, { once: true })
    signal?.addEventListener('abort', abort, { once: true })
    if (lifetime.aborted || signal?.aborted) abort()
    const check = () => { this.check(snapshot); if (controller.signal.aborted) throw cancelled() }
    try {
      const headers = new Headers({ Authorization: 'Bearer ' + snapshot.props.token })
      if (body !== undefined) headers.set('Content-Type', 'application/json')
      if (idempotencyKey !== undefined) headers.set('Idempotency-Key', idempotencyKey)
      let observedCode = '', observedStatus, observedRequestId
      const fetchRequest = this.fetchImpl
      const transport = async (url, options) => {
        const response = await fetchRequest(url, options)
        observedStatus = response?.status
        const echoed = response?.headers?.get('X-Request-Id')
        observedRequestId = uuid(echoed) ? echoed : options.headers.get('X-Request-Id')
        if (uncertainOutcome) observedCode = await unknownCode(response, options.signal)
        return response
      }
      try {
        const result = await requestWithContext({ props: snapshot.props, url: this.apiBase + path,
          options: { method, headers, signal: controller.signal, body: body === undefined ? undefined : JSON.stringify(body) },
          fetchImpl: transport, check, responseType: text ? 'text' : 'json', maxBytes: MAX_BYTES })
        check()
        try {
          requireShape(observedStatus === expectedStatus)
          return decode ? decode(result) : result
        } catch {
          throw new MicroSdkError('PROTOCOL', { status: observedStatus, requestId: observedRequestId, code: 'INVALID_BUSINESS_RESPONSE' })
        }
      } catch (error) {
        check()
        if (!(error instanceof MicroSdkError)) throw error
        if (error.status === 401) this.stop()
        const status = error.kind === 'NETWORK' ? 503 : error.kind === 'PROTOCOL' && error.status !== 413 ? 502 : error.status
        const message = error.status === 401 ? '应用认证已失效，请等待平台刷新授权'
          : error.status === 413 ? '结果超过 4 MiB，请缩小筛选范围后重试'
          : error.kind === 'NETWORK' ? '业务服务暂时无法连接，请稍后重试'
          : error.kind === 'PROTOCOL' ? '业务服务返回的数据无法识别'
          : error.status === 403 ? '当前操作未获授权，请等待平台更新上下文'
          : error.status === 409 ? '工单已更新，请刷新并核对后重试' : '操作未完成，请稍后重试'
        const failure = new BusinessError(status, message, observedCode || error.code, error.kind, error.requestId)
        if (uncertainOutcome && (error.kind === 'NETWORK' || error.kind === 'PROTOCOL' || error.status >= 500)) {
          failure.outcome = uncertainOutcome === 'REMINDER_RESULT_UNKNOWN' && observedCode === 'REMINDER_PREPARATION_UNKNOWN'
            ? observedCode : uncertainOutcome
        }
        throw failure
      }
    } finally {
      lifetime.removeEventListener('abort', abort)
      signal?.removeEventListener('abort', abort)
    }
  }
  list(filters, signal) {
    const params = new URLSearchParams({ page: String(filters.page ?? 1), size: String(filters.size ?? 20) })
    if (filters.status && filters.status !== 'ALL') params.set('status', filters.status)
    if (filters.priority && filters.priority !== 'ALL') params.set('priority', filters.priority)
    return this.request('/tickets?' + params, { signal, decode: value => pageResponse(value, ticketResponse, 100) })
  }
  detail(id, signal) { return this.request('/tickets/' + encodeURIComponent(id), { signal, decode: value => ticketFor(id, value) }) }
  async create(body, idempotencyKey, signal) {
    if (!validIdempotencyKey(idempotencyKey)) throw new TypeError('创建工单需要有效的操作事件键')
    return this.request('/tickets', { method: 'POST', body, idempotencyKey, signal, uncertainOutcome: 'CREATE_RESULT_UNKNOWN',
      decode: value => { const ticket = ticketResponse(value); requireShape(ticket.version === 1 && ticket.status === 'OPEN'); return ticket }, expectedStatus: 201 })
  }
  updateTicket(id, body) {
    return this.request('/tickets/' + encodeURIComponent(id), { method: 'PATCH', body, uncertainOutcome: 'WRITE_RESULT_UNKNOWN',
      decode: value => { const ticket = ticketFor(id, value); requireShape(ticket.version === body.version + 1); return ticket } })
  }
  remove(id, version) { return this.request('/tickets/' + encodeURIComponent(id), { method: 'DELETE', body: { version }, uncertainOutcome: 'WRITE_RESULT_UNKNOWN', decode: deleteResponse }) }
  assignees(action, organizationId, signal, { q = '', page = 1, size = 50 } = {}) {
    const params = new URLSearchParams({ action, organizationId, q, page: String(page), size: String(size) })
    return this.request('/assignees?' + params, { signal, decode: value => pageResponse(value, memberResponse, 50) })
  }
  exportCsv(filters) {
    const params = new URLSearchParams()
    if (filters.status && filters.status !== 'ALL') params.set('status', filters.status)
    if (filters.priority && filters.priority !== 'ALL') params.set('priority', filters.priority)
    return this.request('/tickets/export.csv' + (params.size ? '?' + params : ''), { text: true })
  }
  async remind(id, idempotencyKey, signal) {
    if (!validIdempotencyKey(idempotencyKey)) throw new TypeError('工单提醒需要有效的操作事件键')
    return this.request('/tickets/' + encodeURIComponent(id) + '/remind', { method: 'POST', body: { idempotencyKey }, signal, uncertainOutcome: 'REMINDER_RESULT_UNKNOWN', decode: reminderResponse })
  }
}
