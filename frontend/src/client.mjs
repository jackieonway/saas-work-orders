import { validContext, MAX_BYTES } from './domain.mjs'

export class BusinessError extends Error {
  constructor(status, message, code = '') { super(message); this.name = 'BusinessError'; this.status = status; this.code = code }
}
const cancelled = () => new DOMException('工单上下文已变更', 'AbortError')

/** 只发送捕获的应用 token；旧 fetch/json 即使忽略取消，也不能返回到新上下文。 */
export class WorkOrdersClient {
  constructor({ apiBase = '/work-orders-api', fetchImpl = fetch, onInvalidate = () => {} } = {}) {
    this.apiBase = apiBase.replace(/\/$/, '')
    this.fetchImpl = fetchImpl
    this.onInvalidate = onInvalidate
    this.props = null
    this.generation = 0
    this.controller = null
    this.unbind = null
  }
  current() { return validContext(this.props) && !this.controller?.signal.aborted ? this.props : null }
  update(props) {
    if (!validContext(props)) throw cancelled()
    this.stop()
    this.props = props
    const lifetime = new AbortController()
    this.controller = lifetime
    const abort = () => { if (this.props === props) this.stop() }
    props.signal.addEventListener('abort', abort, { once: true })
    this.unbind = () => props.signal.removeEventListener('abort', abort)
    if (props.signal.aborted) abort()
  }
  stop() {
    this.generation++
    this.controller?.abort()
    this.unbind?.()
    this.unbind = null
    this.props = null
    this.controller = null
    this.onInvalidate()
  }
  capture() {
    const props = this.current()
    if (!props) throw cancelled()
    return { props, generation: this.generation }
  }
  isCurrent(snapshot) { return snapshot.generation === this.generation && snapshot.props === this.current() }
  check(snapshot) { if (!this.isCurrent(snapshot)) throw cancelled() }
  async request(path, { method = 'GET', body, signal, text = false } = {}) {
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
      let response
      const fetchRequest = this.fetchImpl
      try { response = await fetchRequest(this.apiBase + path, { method, headers, signal: controller.signal,
        credentials: 'omit', body: body === undefined ? undefined : JSON.stringify(body) }) }
      catch (error) { check(); if (error.name === 'AbortError') throw error; throw new BusinessError(503, '业务服务暂时无法连接，请稍后重试') }
      check()
      if (response.status === 401) {
        this.stop()
        throw new BusinessError(401, '应用认证已失效，请等待平台刷新授权')
      }
      const raw = await readLimited(response, check, text)
      check()
      let parsed = null
      if (!text || !response.ok) {
        try { parsed = raw ? JSON.parse(raw) : null } catch { if (response.ok) throw new BusinessError(502, '业务服务返回的数据无法识别') }
      }
      if (!response.ok) throw new BusinessError(response.status,
        typeof parsed?.message === 'string' ? parsed.message.replaceAll(snapshot.props.token, '[认证信息]') : '操作未完成，请稍后重试', parsed?.code)
      return text ? raw : parsed
    } finally {
      lifetime.removeEventListener('abort', abort)
      signal?.removeEventListener('abort', abort)
    }
  }
  list(filters, signal) {
    const params = new URLSearchParams({ page: String(filters.page ?? 1), size: String(filters.size ?? 20) })
    if (filters.status && filters.status !== 'ALL') params.set('status', filters.status)
    if (filters.priority && filters.priority !== 'ALL') params.set('priority', filters.priority)
    return this.request('/tickets?' + params, { signal })
  }
  detail(id, signal) { return this.request('/tickets/' + encodeURIComponent(id), { signal }) }
  create(body) { return this.request('/tickets', { method: 'POST', body }) }
  updateTicket(id, body) { return this.request('/tickets/' + encodeURIComponent(id), { method: 'PATCH', body }) }
  remove(id, version) { return this.request('/tickets/' + encodeURIComponent(id), { method: 'DELETE', body: { version } }) }
  assignees(action, organizationId, signal, { q = '', page = 1, size = 50 } = {}) {
    const params = new URLSearchParams({ action, organizationId, q, page: String(page), size: String(size) })
    return this.request('/assignees?' + params, { signal })
  }
  exportCsv(filters) {
    const params = new URLSearchParams()
    if (filters.status && filters.status !== 'ALL') params.set('status', filters.status)
    if (filters.priority && filters.priority !== 'ALL') params.set('priority', filters.priority)
    return this.request('/tickets/export.csv' + (params.size ? '?' + params : ''), { text: true })
  }
  remind(id, idempotencyKey) { return this.request('/tickets/' + encodeURIComponent(id) + '/remind', { method: 'POST', body: { idempotencyKey } }) }
}

async function readLimited(response, check, preserveBom) {
  const declared = Number(response.headers.get('Content-Length'))
  if (declared > MAX_BYTES) throw new BusinessError(413, '结果超过 4 MiB，请缩小筛选范围后重试')
  if (!response.body?.getReader) {
    const text = await response.text()
    check()
    if (new TextEncoder().encode(text).byteLength > MAX_BYTES) throw new BusinessError(413, '结果超过 4 MiB，请缩小筛选范围后重试')
    return text
  }
  const reader = response.body.getReader(), decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: preserveBom })
  const parts = []
  let bytes = 0
  try {
    for (;;) {
      const next = await reader.read()
      check()
      if (next.done) break
      bytes += next.value.byteLength
      if (bytes > MAX_BYTES) { await reader.cancel(); throw new BusinessError(413, '结果超过 4 MiB，请缩小筛选范围后重试') }
      parts.push(decoder.decode(next.value, { stream: true }))
    }
    parts.push(decoder.decode())
    return parts.join('')
  } finally { reader.releaseLock() }
}
