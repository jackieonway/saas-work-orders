import { ApiError, reject } from './errors.mjs'

export const validId = (value) => typeof value === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)
const id = (value) => validId(value) ? value.toLowerCase() : null

export function bearer(headers) {
  const header = headers.authorization
  if (typeof header !== 'string' || !/^Bearer [^\s]+$/.test(header) || header.length > 8192) {
    reject(401, 'INVALID_APP_TOKEN', '需要有效应用令牌')
  }
  return header.slice(7)
}

async function readJson(response, signal, maxBytes) {
  if (!/^application\/json(?:;|$)/i.test(response.headers.get('content-type') ?? '') ||
      Number(response.headers.get('content-length')) > maxBytes) {
    await response.body?.cancel()
    reject(503, 'PLATFORM_UNAVAILABLE', '平台授权响应无效，请稍后重试')
  }
  const reader = response.body?.getReader()
  if (!reader) reject(503, 'PLATFORM_UNAVAILABLE', '平台授权响应无效，请稍后重试')
  const parts = []
  let bytes = 0
  try {
    while (true) {
      signal.throwIfAborted()
      const { done, value } = await reader.read()
      if (done) break
      bytes += value.byteLength
      if (bytes > maxBytes) {
        await reader.cancel()
        reject(503, 'PLATFORM_UNAVAILABLE', '平台授权响应超过容量限制')
      }
      parts.push(value)
    }
    try { return JSON.parse(Buffer.concat(parts).toString('utf8')) }
    catch { reject(503, 'PLATFORM_UNAVAILABLE', '平台授权响应无效，请稍后重试') }
  } finally { reader.releaseLock() }
}

export async function platformJson(config, path, options, fetchImpl = fetch, parentSignal, passErrors = false) {
  const signal = parentSignal ? AbortSignal.any([parentSignal, AbortSignal.timeout(config.platformTimeoutMs)])
    : AbortSignal.timeout(config.platformTimeoutMs)
  try {
    const response = await fetchImpl(new URL(path, config.platformOrigin), {
      ...options, redirect: 'error', signal,
      headers: { Accept: 'application/json', ...options.headers },
    })
    if (response.status !== 200) {
      await response.body?.cancel()
      if (response.status === 401) reject(401, 'INVALID_APP_TOKEN', '应用令牌或平台会话已失效')
      if (response.status === 403) reject(403, 'DATA_DENIED', '当前操作或目标未获授权')
      if (passErrors && response.status === 404) reject(404, 'MEMBER_NOT_FOUND', '处理人不存在或不属于该组织的有效成员')
      if (passErrors && response.status === 409) reject(409, 'IDEMPOTENCY_CONFLICT', '提醒键已用于不同的提醒，请重新发起独立提醒')
      if (passErrors && response.status === 400) reject(400, 'VALIDATION_ERROR', '目标无法接收提醒或请求无效')
      reject(503, 'PLATFORM_UNAVAILABLE', '平台授权服务暂时不可用，请稍后重试')
    }
    return await readJson(response, signal, config.maxPlatformBodyBytes)
  } catch (error) {
    if (error instanceof ApiError) throw error
    reject(503, 'PLATFORM_UNAVAILABLE', '平台授权服务暂时不可用，请稍后重试')
  }
}

export function grantFor(context, resource, action) {
  if (!Array.isArray(context.permissions) || !context.permissions.includes(`${resource}:${action}`)) {
    reject(403, 'OPERATION_DENIED', '当前用户没有该操作权限')
  }
  const matches = Array.isArray(context.dataGrants)
    ? context.dataGrants.filter((item) => item?.resource === resource && item?.action === action) : []
  const grant = matches.length === 1 ? matches[0] : null
  if (!grant || id(grant.tenantId) !== context.tenant.id || id(grant.subjectUserId) !== context.user.id ||
      !['SELF', 'ORG', 'ORG_SUBTREE', 'TENANT'].includes(grant.scope) || !Array.isArray(grant.organizationIds) ||
      grant.organizationIds.length > 10000 || !grant.organizationIds.every(validId)) {
    reject(403, 'DATA_DENIED', '缺少有效的独立数据授权')
  }
  const organizationIds = [...new Set(grant.organizationIds.map((item) => item.toLowerCase()))]
  if ((grant.scope === 'ORG' && (organizationIds.length !== 1 || organizationIds[0] !== context.organization.id)) ||
      (grant.scope === 'ORG_SUBTREE' && !organizationIds.includes(context.organization.id)) ||
      (['SELF', 'TENANT'].includes(grant.scope) && organizationIds.length !== 0)) {
    reject(403, 'DATA_DENIED', '数据授权范围无效')
  }
  return { ...grant, organizationIds }
}

/** Fresh protected introspection and context on every request. No positive cache. */
export async function authorize(config, token, action, fetchImpl = fetch, signal, metadata = {}) {
  const intro = await platformJson(config, '/oauth/introspect', {
    method: 'POST',
    headers: { authorization: `Basic ${Buffer.from(`${config.clientId}:${config.clientSecret}`).toString('base64')}`,
      'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ token }),
  }, fetchImpl, signal)
  if (intro?.active !== true || intro.aud !== config.appCode || !id(intro.sub) || !id(intro.tenant_id) ||
      !id(intro.organization_id) || !Number.isSafeInteger(intro.exp) || intro.exp * 1000 <= Date.now() ||
      intro.token_type !== 'Bearer') reject(401, 'INVALID_APP_TOKEN', '应用令牌无效或受众不匹配')
  const context = await platformJson(config, `/api/apps/${config.appCode}/context`, {
    headers: { authorization: `Bearer ${token}` },
  }, fetchImpl, signal)
  const expiresAt = Date.parse(context?.expiresAt)
  if (context?.contractVersion !== '1.0' || context.appCode !== config.appCode ||
      !/^[0-9a-f]{64}$/i.test(context.contextVersion ?? '') || !Number.isFinite(expiresAt) ||
      expiresAt <= Date.now() || expiresAt > Date.now() + 30000 || expiresAt > intro.exp * 1000 + 999 ||
      id(context.user?.id) !== id(intro.sub) || id(context.tenant?.id) !== id(intro.tenant_id) ||
      id(context.organization?.id) !== id(intro.organization_id) || !validId(context.application?.id) ||
      typeof context.user?.username !== 'string' || typeof context.organization?.name !== 'string' ||
      !context.organization.name.trim() || Array.from(context.organization.name).length > 100 ||
      (context.user.displayName != null && (typeof context.user.displayName !== 'string' || Array.from(context.user.displayName).length > 100))) {
    reject(401, 'INVALID_CONTEXT', '平台授权上下文失效或身份不一致')
  }
  context.user.id = id(context.user.id)
  context.tenant.id = id(context.tenant.id)
  context.organization.id = id(context.organization.id)
  const grant = grantFor(context, 'ticket', action)
  metadata.tenantId = context.tenant.id
  metadata.organizationId = context.organization.id
  return { context, grant, token, expiresAt: Math.min(expiresAt, intro.exp * 1000) }
}

export function assertFresh(auth) {
  if (auth.expiresAt <= Date.now()) reject(401, 'INVALID_CONTEXT', '授权上下文已到期，请刷新后重试')
}

export function scopeSql(auth, start = 1) {
  const values = [auth.context.tenant.id]
  let sql = `tenant_id=$${start} AND deleted_at IS NULL`
  switch (auth.grant.scope) {
    case 'SELF': sql += ` AND owner_user_id=$${start + 1}`; values.push(auth.context.user.id); break
    case 'ORG': sql += ` AND organization_id=$${start + 1}`; values.push(auth.context.organization.id); break
    case 'ORG_SUBTREE': sql += ` AND organization_id=ANY($${start + 1}::uuid[])`; values.push(auth.grant.organizationIds); break
    case 'TENANT': break
    default: reject(403, 'DATA_DENIED', '数据授权范围无效')
  }
  return { sql, values }
}

export function directoryScope(auth, organizationId) {
  const { grant, context } = auth
  const allowed = grant.scope === 'TENANT' || (grant.scope === 'ORG_SUBTREE' ? grant.organizationIds.includes(organizationId)
    : organizationId === context.organization.id)
  if (!allowed) reject(403, 'DATA_DENIED', '当前数据授权不允许选择该组织的处理人；请切换到该组织')
}

export async function member(config, auth, organizationId, userId, action, fetchImpl, signal) {
  directoryScope(auth, organizationId)
  const query = new URLSearchParams({ resource: 'ticket', action, organizationId })
  const result = await platformJson(config, `/api/apps/${config.appCode}/members/${userId}?${query}`, {
    headers: { authorization: `Bearer ${auth.token}` },
  }, fetchImpl, signal, true)
  if (id(result?.id) !== userId || id(result.organizationId) !== organizationId ||
      typeof result.displayName !== 'string' || !result.displayName.trim() || Array.from(result.displayName).length > 100) {
    reject(503, 'PLATFORM_UNAVAILABLE', '平台处理人响应无效')
  }
  return result.displayName
}
