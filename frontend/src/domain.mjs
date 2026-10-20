export const APP_CODE = 'work-orders'
export const MAX_BYTES = 4 * 1024 * 1024
export const validIdempotencyKey = value => typeof value === 'string' && /^[A-Za-z0-9._:-]{1,128}$/.test(value)
export const STATUSES = { OPEN: '待处理', IN_PROGRESS: '处理中', RESOLVED: '已解决' }
export const PRIORITIES = { LOW: '低', NORMAL: '普通', HIGH: '高' }
export const SCOPE_LABELS = { SELF: '我创建的工单', ORG: '当前组织', ORG_SUBTREE: '当前组织及下级组织', TENANT: '当前租户' }
const STATUS_TRANSITIONS = { OPEN: ['OPEN', 'IN_PROGRESS'], IN_PROGRESS: ['IN_PROGRESS', 'RESOLVED'], RESOLVED: ['RESOLVED', 'IN_PROGRESS'] }

export function statusChoices(status) {
  return Object.fromEntries((Object.hasOwn(STATUS_TRANSITIONS, status) ? STATUS_TRANSITIONS[status] : []).map((value) => [value, STATUSES[value]]))
}

export function validContext(props, now = Date.now()) {
  return props?.contractVersion === '2.0' && props.appCode === APP_CODE && props.basePath === '/micro/' + APP_CODE &&
    Number.isSafeInteger(props.contextEpoch) && props.contextEpoch > 0 && !!props.contextVersion &&
    typeof props.token === 'string' && !!props.token && !!props.signal && !props.signal.aborted &&
    Date.parse(props.expiresAt) > now && Date.parse(props.tokenExpiresAt) > now &&
    !!props.user?.id && !!props.tenant?.id && !!props.organization?.id &&
    Array.isArray(props.permissions) && Array.isArray(props.dataGrants) &&
    (props.organizationScopeIds == null || (Array.isArray(props.organizationScopeIds) &&
      props.organizationScopeIds.length <= 10000 && props.organizationScopeIds.every((value) => typeof value === 'string' && !!value) &&
      props.organizationScopeIds.includes(props.organization.id)))
}

export function sameIdentity(left, right) {
  return !!left && !!right && left.user.id === right.user.id && left.tenant.id === right.tenant.id &&
    left.organization.id === right.organization.id && left.appCode === right.appCode
}

export function grants(props, resource, action) {
  if (!validContext(props) || !props.permissions.includes(resource + ':' + action)) return []
  return props.dataGrants.filter((grant) => grant.resource === resource && grant.action === action &&
    grant.subjectUserId === props.user.id && grant.tenantId === props.tenant.id && Object.hasOwn(SCOPE_LABELS, grant.scope))
}

function withinOrganizationCeiling(props, organizationId) {
  if (props.organizationScopeIds === null) return true
  return Array.isArray(props.organizationScopeIds) ? props.organizationScopeIds.includes(organizationId)
    : organizationId === props.organization.id
}

function organizationAllowed(grant, organizationId, props) {
  if (!withinOrganizationCeiling(props, organizationId)) return false
  if (grant.scope === 'TENANT') return true
  if (grant.scope === 'ORG') return organizationId === props.organization.id
  return grant.scope === 'ORG_SUBTREE' && (organizationId === props.organization.id || (Array.isArray(grant.organizationIds) && grant.organizationIds.includes(organizationId)))
}

export function canTicket(props, action, ticket) {
  const available = grants(props, 'ticket', action)
  if (!ticket) return available.length > 0
  if (!available.length || ticket.tenantId !== props?.tenant.id || !withinOrganizationCeiling(props, ticket.organizationId)) return false
  return available.some((grant) => grant.scope === 'SELF' ? ticket.ownerUserId === props.user.id &&
    Array.isArray(grant.organizationIds) && (grant.organizationIds.length === 0 || grant.organizationIds.includes(ticket.organizationId))
    : organizationAllowed(grant, ticket.organizationId, props))
}

export function canAssign(props, ticket) {
  if (!ticket) return canTicket(props, 'create')
  return canTicket(props, 'update', ticket) && (ticket.organizationId === props.organization.id ||
    grants(props, 'ticket', 'update').some((grant) => organizationAllowed(grant, ticket.organizationId, props)))
}

export function canRemind(props, ticket) {
  if (!ticket || !canTicket(props, 'read', ticket)) return false
  const userId = ticket.assigneeUserId || ticket.ownerUserId
  return grants(props, 'notification', 'publish').some((grant) => grant.scope === 'SELF'
    ? userId === props.user.id && ticket.organizationId === props.organization.id
    : organizationAllowed(grant, ticket.organizationId, props))
}

export function visibleScope(props, action = 'read') {
  const available = grants(props, 'ticket', action)
  const scope = ['TENANT', 'ORG_SUBTREE', 'ORG', 'SELF'].find((scope) => available.some((grant) => grant.scope === scope))
  return scope === 'TENANT' && props.organizationScopeIds !== null
    ? (props.organizationScopeIds === undefined ? 'ORG' : 'ORG_SUBTREE') : scope
}

export function routeView(routePath = '') {
  const base = '/micro/' + APP_CODE
  const relative = routePath === base || routePath.startsWith(base + '/') ? routePath.slice(base.length).replace(/\/$/, '') : ''
  if (relative === '/ticket-create') return { kind: 'create' }
  const match = /^\/tickets\/([a-zA-Z0-9-]{1,128})$/.exec(relative)
  return match ? { kind: 'detail', id: match[1] } : { kind: 'list' }
}

export function ticketDraft(ticket) {
  return { title: ticket?.title ?? '', description: ticket?.description ?? '', priority: ticket?.priority ?? 'NORMAL',
    status: ticket?.status ?? 'OPEN', assigneeUserId: ticket?.assigneeUserId ?? '', version: ticket?.version ?? null }
}

export function validateDraft(draft) {
  if (!draft.title.trim()) return '请输入工单标题'
  if (Array.from(draft.title.trim()).length > 200) return '工单标题最多 200 个字符'
  if (Array.from(draft.description).length > 8192) return '描述最多 8192 个字符'
  if (!Object.hasOwn(PRIORITIES, draft.priority) || !Object.hasOwn(STATUSES, draft.status)) return '请选择有效的状态与优先级'
  return null
}
