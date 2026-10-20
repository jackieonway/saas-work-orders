import { validContext, validIdempotencyKey, PRIORITIES } from './domain.mjs'

/** Only the business identity is retained; renewing credentials is not a new event. */
export function identityKey(props) {
  return validContext(props) ? JSON.stringify([props.appCode, props.tenant.id, props.organization.id, props.user.id]) : null
}

function requiredIdentity(props) {
  const identity = identityKey(props)
  if (identity === null) throw new DOMException('工单上下文已变更', 'AbortError')
  return identity
}

function createBody(body) {
  if (!body || typeof body !== 'object' || Array.isArray(body) ||
      Object.keys(body).some(key => !['title', 'description', 'priority', 'assigneeUserId'].includes(key)) ||
      typeof body.title !== 'string' || (body.description !== undefined && typeof body.description !== 'string') ||
      (body.priority !== undefined && !Object.hasOwn(PRIORITIES, body.priority)) ||
      (body.assigneeUserId !== undefined && body.assigneeUserId !== null && typeof body.assigneeUserId !== 'string')) {
    throw new TypeError('创建意图必须使用工单创建正文')
  }
  // Copy only primitive DTO fields: never retain props, credentials or a mutable draft.
  return Object.freeze(Object.fromEntries(Object.entries(body).filter(([, value]) => value !== undefined)))
}

/** In-memory recovery survives routes and authorization renewal in the mounted iframe.
 * Opaque iframe storage is unavailable; reloads require keeping this view open until resolved.
 */
export class IntentStore {
  #creates = new Map()
  #reminders = new Map()
  #randomUUID
  constructor({ randomUUID = () => globalThis.crypto.randomUUID() } = {}) { this.#randomUUID = randomUUID }
  #key(kind) {
    const key = kind + ':' + this.#randomUUID()
    if (!validIdempotencyKey(key)) throw new TypeError('无法生成有效的操作事件键')
    return key
  }
  getCreate(props) { return this.#creates.get(identityKey(props)) || null }
  prepareCreate(props, body) {
    const identity = requiredIdentity(props), existing = this.#creates.get(identity)
    if (existing) return existing
    const captured = createBody(body)
    const intent = Object.freeze({ kind: 'create', key: this.#key('create'), body: captured })
    this.#creates.set(identity, intent)
    return intent
  }
  settleCreate(props, intent) {
    const identity = identityKey(props)
    return identity !== null && this.#creates.get(identity) === intent ? this.#creates.delete(identity) : false
  }
  getReminder(props, ticketId) { return this.#reminders.get(identityKey(props))?.get(ticketId) || null }
  prepareReminder(props, ticketId) {
    const identity = requiredIdentity(props)
    if (typeof ticketId !== 'string' || !/^[a-zA-Z0-9-]{1,128}$/.test(ticketId)) throw new TypeError('提醒需要有效的工单标识')
    let reminders = this.#reminders.get(identity)
    const existing = reminders?.get(ticketId)
    if (existing) return existing
    const key = this.#key('reminder')
    const intent = Object.freeze({ kind: 'reminder', key, ticketId, body: Object.freeze({ idempotencyKey: key }) })
    if (!reminders) { reminders = new Map(); this.#reminders.set(identity, reminders) }
    reminders.set(ticketId, intent)
    return intent
  }
  settleReminder(props, intent) {
    const identity = identityKey(props), reminders = this.#reminders.get(identity)
    if (identity === null || reminders?.get(intent?.ticketId) !== intent) return false
    reminders.delete(intent.ticketId)
    if (reminders.size === 0) this.#reminders.delete(identity)
    return true
  }
}
