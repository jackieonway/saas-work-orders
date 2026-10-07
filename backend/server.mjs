import http from 'node:http'
import { randomUUID } from 'node:crypto'
import { ApiError, invalid, notFound, reject } from './errors.mjs'
import { authorize as platformAuthorize, assertFresh, bearer, directoryScope, grantFor, member, platformJson, scopeSql, validId } from './auth.mjs'
import { createTelemetry, metricsAllowed } from './observability.mjs'

const PREFIX = '/work-orders-api'
const PRIORITIES = ['LOW', 'NORMAL', 'HIGH']
const STATUSES = ['OPEN', 'IN_PROGRESS', 'RESOLVED']
const TRANSITIONS = { OPEN: ['OPEN', 'IN_PROGRESS'], IN_PROGRESS: ['IN_PROGRESS', 'RESOLVED'], RESOLVED: ['RESOLVED', 'IN_PROGRESS'] }
const CREATE_FIELDS = ['title', 'description', 'priority', 'assigneeUserId']
const UPDATE_FIELDS = ['version', ...CREATE_FIELDS, 'status']
const own = (object, name) => Object.hasOwn(object, name)
const chars = (value) => Array.from(value).length

function json(response, status, body) {
  response.writeHead(status, { 'content-type': 'application/json; charset=utf-8' })
  response.end(JSON.stringify(body))
}

function strict(body, fields) {
  if (!body || typeof body !== 'object' || Array.isArray(body) || Object.keys(body).some((key) => !fields.includes(key))) {
    invalid('请求字段不合法；租户、组织、归属人及显示名称由服务端确定')
  }
}

function text(value, name, maximum, trim = false) {
  if (typeof value !== 'string') invalid(`${name} 必须为文本`)
  const normalized = trim ? value.trim() : value
  if ((trim && !normalized) || chars(normalized) > maximum) invalid(`${name} 必须${trim ? '非空且' : ''}不超过 ${maximum} 字符`)
  return normalized
}

function choice(value, values, name) {
  if (!values.includes(value)) invalid(`${name} 不合法`)
  return value
}

function uuid(value, name) {
  if (!validId(value)) invalid(`${name} 必须为有效 UUID`)
  return value.toLowerCase()
}

function version(value) {
  if (!Number.isSafeInteger(value) || value < 1 || value >= 2147483647) invalid('version 必须为有效正整数')
  return value
}

async function bodyJson(request, maximum) {
  if (!/^application\/json(?:;|$)/i.test(request.headers['content-type'] ?? '')) invalid('请求必须使用 application/json')
  if (Number(request.headers['content-length']) > maximum) reject(413, 'REQUEST_LIMIT', '请求体超过 64 KiB')
  const parts = []
  let size = 0
  for await (const part of request) {
    size += part.length
    if (size > maximum) reject(413, 'REQUEST_LIMIT', '请求体超过 64 KiB')
    parts.push(part)
  }
  try { return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(parts))) }
  catch { invalid('请求体必须为有效 JSON') }
}

function queryParams(params, fields) {
  const seen = new Set()
  for (const key of params.keys()) {
    if (!fields.includes(key) || seen.has(key)) invalid('查询参数不合法')
    seen.add(key)
  }
}

function positive(params, key, fallback, maximum) {
  const value = params.get(key)
  if (value === null) return fallback
  if (!/^\d+$/.test(value) || Number(value) < 1 || Number(value) > maximum) invalid(`${key} 超出允许范围`)
  return Number(value)
}

function filters(auth, params, paged) {
  queryParams(params, ['q', 'status', 'priority', 'organizationId', ...(paged ? ['page', 'size'] : [])])
  const scope = scopeSql(auth)
  let sql = scope.sql
  const values = [...scope.values]
  if (params.has('status')) { values.push(choice(params.get('status'), STATUSES, 'status')); sql += ` AND status=$${values.length}` }
  if (params.has('priority')) { values.push(choice(params.get('priority'), PRIORITIES, 'priority')); sql += ` AND priority=$${values.length}` }
  if (params.has('organizationId')) { values.push(uuid(params.get('organizationId'), 'organizationId')); sql += ` AND organization_id=$${values.length}` }
  const q = params.get('q')?.trim() || ''
  if (chars(q) > 200) invalid('q 不能超过 200 字符')
  if (q) {
    values.push(`%${q.replace(/[\\%_]/g, '\\$&')}%`)
    sql += ` AND (title ILIKE $${values.length} ESCAPE CHR(92) OR description ILIKE $${values.length} ESCAPE CHR(92) OR number ILIKE $${values.length} ESCAPE CHR(92))`
  }
  return { sql, values }
}

function record(row) {
  return {
    id: row.id, number: row.number, title: row.title, description: row.description,
    priority: row.priority, status: row.status, tenantId: row.tenant_id,
    organizationId: row.organization_id, organizationName: row.organization_name,
    ownerUserId: row.owner_user_id, ownerDisplayName: row.owner_display_name,
    assigneeUserId: row.assignee_user_id, assigneeDisplayName: row.assignee_display_name,
    version: row.version, createdAt: row.created_at.toISOString(), updatedAt: row.updated_at.toISOString(),
  }
}

async function load(db, auth, ticketId, lock = false) {
  const { sql, values } = scopeSql(auth)
  values.push(ticketId)
  const rows = await db.query(`SELECT * FROM work_orders WHERE ${sql} AND id=$${values.length}${lock ? ' FOR UPDATE' : ''}`, values)
  if (!rows.rowCount) notFound()
  assertFresh(auth)
  return rows.rows[0]
}

async function transaction(pool, signal, auth, callback) {
  const client = await pool.connect()
  try {
    await client.query('BEGIN')
    const result = await callback(client)
    signal.throwIfAborted()
    assertFresh(auth)
    await client.query('COMMIT')
    return result
  } catch (error) {
    await client.query('ROLLBACK').catch(() => {})
    throw error
  } finally { client.release() }
}

function csvCell(value) {
  let output = String(value ?? '')
  if (/^[\s]*[=+\-@]/u.test(output) || /^[\t\r\n]/.test(output)) output = `'${output}`
  return `"${output.replaceAll('"', '""')}"`
}

async function dispatch(request, response, config, pool, fetchImpl, signal, metadata, health) {
  const authorize = (...args) => platformAuthorize(...args, metadata)
  const url = new URL(request.url, 'http://work-orders.invalid')
  if (url.origin !== 'http://work-orders.invalid' || !url.pathname.startsWith(`${PREFIX}/`)) reject(404, 'ROUTE_NOT_FOUND', '接口不存在')
  const route = url.pathname.slice(PREFIX.length)
  if (route === '/health/live' && request.method === 'GET') return json(response, 200, { status: 'UP' })
  if (route === '/metrics' && request.method === 'GET') {
    if (!metricsAllowed(request.headers.authorization, config.metricsToken)) reject(401, 'METRICS_AUTH_REQUIRED', '监控认证无效')
    response.writeHead(200, { 'content-type': 'text/plain; version=0.0.4; charset=utf-8' })
    return response.end(health.telemetry.render())
  }
  if (route === '/health' && request.method === 'GET') {
    await Promise.all([
      pool.query('SELECT id,tenant_id,organization_id,owner_user_id,version,deleted_at FROM work_orders LIMIT 0')
        .then(() => pool.query('SELECT tenant_id,next_number FROM work_order_counters LIMIT 0'))
        .then(() => { health.database = 'UP' }, () => { health.database = 'DOWN' }),
      (async () => {
        try {
          const probe = await fetchImpl(new URL(`/api/apps/${config.appCode}/context`, config.platformOrigin), {
            method: 'GET', redirect: 'error', headers: { Accept: 'application/json' },
            signal: AbortSignal.any([signal, AbortSignal.timeout(config.platformTimeoutMs)]) })
          await probe.body?.cancel()
          health.platform = probe.status === 401 ? 'UP' : 'DOWN'
        } catch { health.platform = 'DOWN' }
      })(),
    ])
    const status = health.database === 'UP' && health.platform === 'UP' ? 'UP' : 'DOWN'
    return json(response, status === 'UP' ? 200 : 503, { status, service: 'work-orders-api', checks: { database: health.database, platform: health.platform } })
  }
  const token = bearer(request.headers)
  if (route === '/tickets/export.csv' && request.method === 'GET') {
    const auth = await authorize(config, token, 'export', fetchImpl, signal)
    const { sql, values } = filters(auth, url.searchParams, false)
    // Reject known oversized exports before transferring large text fields from
    // PostgreSQL. The final rendered CSV is checked again for quoting/BOM overhead.
    const capacity = await pool.query(`WITH selected AS (
      SELECT number,title,description,organization_name,owner_display_name,assignee_display_name
      FROM work_orders WHERE ${sql} ORDER BY created_at DESC,id LIMIT 1001
    ) SELECT count(*)::integer AS total,COALESCE(sum(octet_length(number)+octet_length(title)+octet_length(description)+
      octet_length(organization_name)+octet_length(owner_display_name)+octet_length(COALESCE(assignee_display_name,''))),0)::text AS minimum_bytes FROM selected`, values)
    assertFresh(auth)
    if (capacity.rows[0].total > 1000) reject(400, 'EXPORT_LIMIT', '单次最多导出 1000 条工单，请缩小筛选范围')
    if (BigInt(capacity.rows[0].minimum_bytes) > 4n * 1024n * 1024n) reject(413, 'EXPORT_LIMIT', 'CSV 超过 4 MiB，请缩小筛选范围')
    const rows = await pool.query(`SELECT * FROM work_orders WHERE ${sql} ORDER BY created_at DESC,id LIMIT 1001`, values)
    assertFresh(auth)
    if (rows.rowCount > 1000) reject(400, 'EXPORT_LIMIT', '单次最多导出 1000 条工单，请缩小筛选范围')
    const priority = { LOW: '低', NORMAL: '普通', HIGH: '高' }
    const status = { OPEN: '待处理', IN_PROGRESS: '处理中', RESOLVED: '已解决' }
    const header = ['编号', '标题', '描述', '优先级', '状态', '组织', '创建人', '处理人', '创建时间', '更新时间']
    const lines = [header.map(csvCell).join(',')]
    for (const row of rows.rows) {
      lines.push([row.number, row.title, row.description, priority[row.priority], status[row.status], row.organization_name,
        row.owner_display_name, row.assignee_display_name, row.created_at.toISOString(), row.updated_at.toISOString()].map(csvCell).join(','))
    }
    const content = `\uFEFF${lines.join('\r\n')}\r\n`
    if (Buffer.byteLength(content, 'utf8') > 4 * 1024 * 1024) reject(413, 'EXPORT_LIMIT', 'CSV 超过 4 MiB，请缩小筛选范围')
    response.writeHead(200, { 'content-type': 'text/csv; charset=utf-8', 'content-disposition': 'attachment; filename="work-orders.csv"' })
    return response.end(content)
  }
  if (route === '/assignees' && request.method === 'GET') {
    queryParams(url.searchParams, ['action', 'organizationId', 'q', 'page', 'size'])
    const action = choice(url.searchParams.get('action'), ['create', 'update'], 'action')
    const auth = await authorize(config, token, action, fetchImpl, signal)
    const organizationId = url.searchParams.has('organizationId') ? uuid(url.searchParams.get('organizationId'), 'organizationId') : auth.context.organization.id
    directoryScope(auth, organizationId)
    const page = positive(url.searchParams, 'page', 1, 1000000)
    const size = positive(url.searchParams, 'size', 20, 50)
    const q = text(url.searchParams.get('q') ?? '', 'q', 100).trim()
    const query = new URLSearchParams({ resource: 'ticket', action, organizationId, page: String(page), size: String(size), q })
    const data = await platformJson(config, `/api/apps/${config.appCode}/members?${query}`, {
      headers: { authorization: `Bearer ${token}` },
    }, fetchImpl, signal, true)
    assertFresh(auth)
    if (!data || data.page !== page || data.size !== size || !Number.isSafeInteger(data.total) || data.total < 0 ||
        !Array.isArray(data.records) || data.records.length > size || data.records.some((item) => !validId(item?.id) ||
        item.organizationId?.toLowerCase() !== organizationId || typeof item.displayName !== 'string' || !item.displayName.trim() || chars(item.displayName) > 100)) {
      reject(503, 'PLATFORM_UNAVAILABLE', '平台处理人列表响应无效')
    }
    return json(response, 200, { ...data, records: data.records.map((item) => ({ userId: item.id.toLowerCase(), displayName: item.displayName, organizationId })) })
  }
  if (route === '/tickets' && request.method === 'GET') {
    const auth = await authorize(config, token, 'read', fetchImpl, signal)
    const { sql, values } = filters(auth, url.searchParams, true)
    const page = positive(url.searchParams, 'page', 1, 1000000)
    const size = positive(url.searchParams, 'size', 20, 100)
    const [count, rows] = await Promise.all([
      pool.query(`SELECT count(*)::integer AS total FROM work_orders WHERE ${sql}`, values),
      pool.query(`SELECT * FROM work_orders WHERE ${sql} ORDER BY created_at DESC,id LIMIT $${values.length + 1} OFFSET $${values.length + 2}`, [...values, size, (page - 1) * size]),
    ])
    assertFresh(auth)
    return json(response, 200, { records: rows.rows.map(record), total: count.rows[0].total, page, size })
  }
  if (route === '/tickets' && request.method === 'POST') {
    const auth = await authorize(config, token, 'create', fetchImpl, signal)
    const body = await bodyJson(request, config.maxBodyBytes)
    strict(body, CREATE_FIELDS)
    const title = text(body.title, 'title', 200, true)
    const description = text(own(body, 'description') ? body.description : '', 'description', 8192)
    const priority = choice(own(body, 'priority') ? body.priority : 'NORMAL', PRIORITIES, 'priority')
    const assignee = body.assigneeUserId == null ? null : uuid(body.assigneeUserId, 'assigneeUserId')
    const assigneeName = assignee ? await member(config, auth, auth.context.organization.id, assignee, 'create', fetchImpl, signal) : null
    const result = await transaction(pool, signal, auth, async (client) => {
      assertFresh(auth)
      const counter = await client.query('INSERT INTO work_order_counters(tenant_id,next_number) VALUES($1,1) ON CONFLICT(tenant_id) DO UPDATE SET next_number=work_order_counters.next_number+1 RETURNING next_number', [auth.context.tenant.id])
      const number = `WO-${String(counter.rows[0].next_number).padStart(6, '0')}`
      const rows = await client.query('INSERT INTO work_orders(id,number,tenant_id,organization_id,organization_name,owner_user_id,owner_display_name,assignee_user_id,assignee_display_name,title,description,priority) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING *',
        [randomUUID(), number, auth.context.tenant.id, auth.context.organization.id, auth.context.organization.name,
          auth.context.user.id, auth.context.user.displayName?.trim() || auth.context.user.username, assignee, assigneeName, title, description, priority])
      assertFresh(auth)
      return record(rows.rows[0])
    })
    return json(response, 201, result)
  }
  const match = /^\/tickets\/([^/]+)(\/remind)?$/.exec(route)
  if (!match) reject(404, 'ROUTE_NOT_FOUND', '接口不存在')
  const ticketId = uuid(match[1], 'id')
  queryParams(url.searchParams, [])
  if (!match[2] && request.method === 'GET') {
    const auth = await authorize(config, token, 'read', fetchImpl, signal)
    return json(response, 200, record(await load(pool, auth, ticketId)))
  }
  if (!match[2] && request.method === 'PATCH') {
    const auth = await authorize(config, token, 'update', fetchImpl, signal)
    const body = await bodyJson(request, config.maxBodyBytes)
    strict(body, UPDATE_FIELDS)
    version(body.version)
    if (!Object.keys(body).some((key) => key !== 'version')) invalid('请提供至少一个待更新字段')
    const result = await transaction(pool, signal, auth, async (client) => {
      const row = await load(client, auth, ticketId, true)
      if (row.version !== body.version) reject(409, 'VERSION_CONFLICT', '工单已被修改，请刷新后重试')
      const title = own(body, 'title') ? text(body.title, 'title', 200, true) : row.title
      const description = own(body, 'description') ? text(body.description, 'description', 8192) : row.description
      const priority = own(body, 'priority') ? choice(body.priority, PRIORITIES, 'priority') : row.priority
      const status = own(body, 'status') ? choice(body.status, STATUSES, 'status') : row.status
      if (!TRANSITIONS[row.status].includes(status)) invalid('状态只允许开始处理、解决或重新打开已解决工单')
      let assignee = row.assignee_user_id
      let assigneeName = row.assignee_display_name
      if (own(body, 'assigneeUserId')) {
        const target = body.assigneeUserId === null ? null : uuid(body.assigneeUserId, 'assigneeUserId')
        if (target !== assignee) {
          directoryScope(auth, row.organization_id)
          assigneeName = target ? await member(config, auth, row.organization_id, target, 'update', fetchImpl, signal) : null
          assignee = target
        }
      }
      assertFresh(auth)
      const rows = await client.query('UPDATE work_orders SET title=$2,description=$3,priority=$4,status=$5,assignee_user_id=$6,assignee_display_name=$7,version=version+1,updated_at=statement_timestamp() WHERE id=$1 AND tenant_id=$8 AND deleted_at IS NULL AND version=$9 RETURNING *',
        [ticketId, title, description, priority, status, assignee, assigneeName, auth.context.tenant.id, body.version])
      if (!rows.rowCount) reject(409, 'VERSION_CONFLICT', '工单已被修改，请刷新后重试')
      return record(rows.rows[0])
    })
    return json(response, 200, result)
  }
  if (!match[2] && request.method === 'DELETE') {
    const auth = await authorize(config, token, 'delete', fetchImpl, signal)
    const body = await bodyJson(request, config.maxBodyBytes)
    strict(body, ['version'])
    version(body.version)
    await transaction(pool, signal, auth, async (client) => {
      const row = await load(client, auth, ticketId, true)
      if (row.version !== body.version) reject(409, 'VERSION_CONFLICT', '工单已被修改，请刷新后重试')
      await client.query('UPDATE work_orders SET deleted_at=statement_timestamp(),updated_at=statement_timestamp(),version=version+1 WHERE id=$1 AND tenant_id=$2 AND deleted_at IS NULL AND version=$3', [ticketId, auth.context.tenant.id, body.version])
    })
    return json(response, 200, { ok: true })
  }
  if (match[2] && request.method === 'POST') {
    const auth = await authorize(config, token, 'read', fetchImpl, signal)
    const body = await bodyJson(request, config.maxBodyBytes)
    strict(body, ['idempotencyKey'])
    if (typeof body.idempotencyKey !== 'string' || !/^[A-Za-z0-9._:-]{1,128}$/.test(body.idempotencyKey)) invalid('idempotencyKey 必须为 1 到 128 位 ASCII 字母、数字或 ._:-')
    const row = await load(pool, auth, ticketId)
    const grant = grantFor(auth.context, 'notification', 'publish')
    const recipient = row.assignee_user_id ?? row.owner_user_id
    const allowed = grant.scope === 'TENANT' || (grant.scope === 'ORG_SUBTREE' ? grant.organizationIds.includes(row.organization_id)
      : row.organization_id === auth.context.organization.id && (grant.scope !== 'SELF' || recipient === auth.context.user.id))
    if (!allowed) reject(403, 'DATA_DENIED', '通知数据授权不允许提醒该接收人')
    const result = await platformJson(config, `/api/apps/${config.appCode}/notifications`, {
      method: 'POST', headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
      body: JSON.stringify({ idempotencyKey: body.idempotencyKey, target: { kind: 'USER', userId: recipient, organizationId: row.organization_id },
        type: 'INFO', title: `工单提醒 ${row.number}`, content: `请查看工单 ${row.number} 的详情并处理。`, link: `/micro/work-orders/tickets/${row.id}` }),
    }, fetchImpl, signal, true)
    if (!validId(result?.publicationId) || !Number.isSafeInteger(result.recipientCount) || result.recipientCount !== 1 ||
        result.deliveryCount !== 1 || typeof result.duplicate !== 'boolean' || !Number.isFinite(Date.parse(result.createdAt))) {
      reject(503, 'PLATFORM_UNAVAILABLE', '平台提醒结果无效，请保留同一提醒键重试')
    }
    return json(response, 200, result)
  }
  reject(405, 'METHOD_NOT_ALLOWED', '请求方法不支持')
}

export function createServer({ config, pool, fetchImpl = fetch, logger = console }) {
  const health = { database: 'UNKNOWN', platform: 'UNKNOWN' }
  const telemetry = createTelemetry({ service: 'work-orders', logger, routeFor: (request) => {
    let path
    try { path = new URL(request.url, 'http://work-orders.invalid').pathname } catch { return 'OTHER' }
    if (['/health', '/health/live', '/metrics', '/tickets', '/tickets/export.csv', '/assignees'].some((route) => path === PREFIX + route)) return path
    if (/^\/work-orders-api\/tickets\/[^/]+\/remind$/.test(path)) return PREFIX + '/tickets/:id/remind'
    return /^\/work-orders-api\/tickets\/[^/]+$/.test(path) ? PREFIX + '/tickets/:id' : 'OTHER'
  }, gauges: () => ({ saas_database_up: Number(health.database === 'UP'), saas_platform_up: Number(health.platform === 'UP'),
    saas_health_up: Number(health.database === 'UP' && health.platform === 'UP') }) })
  health.telemetry = telemetry
  const server = http.createServer((request, response) => {
    const metadata = telemetry.observe(request, response)
    const tracedFetch = (url, options) => fetchImpl(url, { ...options, headers: { ...options.headers, 'X-Request-Id': metadata.requestId } })
    const abort = new AbortController()
    request.once('aborted', () => abort.abort())
    response.once('close', () => { if (!response.writableFinished) abort.abort() })
    response.setHeader('cache-control', 'no-store')
    response.setHeader('x-content-type-options', 'nosniff')
    response.setHeader('content-security-policy', "default-src 'none'; frame-ancestors 'none'")
    const origin = request.headers.origin
    if (origin) {
      if (!config.allowedOrigins.includes(origin)) return json(response, 403, { code: 'ORIGIN_DENIED', message: '来源未获允许' })
      response.setHeader('access-control-allow-origin', origin)
      response.setHeader('vary', 'Origin')
    }
    if (request.method === 'OPTIONS') {
      const method = request.headers['access-control-request-method']
      const headers = (request.headers['access-control-request-headers'] ?? '').split(',').map((value) => value.trim().toLowerCase()).filter(Boolean)
      if (!origin || !['GET', 'POST', 'PATCH', 'DELETE'].includes(method) || headers.some((value) => !['authorization', 'content-type'].includes(value))) {
        return json(response, 403, { code: 'ORIGIN_DENIED', message: '预检请求未获允许' })
      }
      response.writeHead(204, { 'access-control-allow-methods': 'GET, POST, PATCH, DELETE, OPTIONS', 'access-control-allow-headers': 'Authorization, Content-Type' })
      return response.end()
    }
    dispatch(request, response, config, pool, tracedFetch, abort.signal, metadata, health).catch((error) => {
      if (response.destroyed || response.writableEnded) return
      if (error instanceof ApiError) return json(response, error.status, { code: error.code, message: error.message })
      logger.error(JSON.stringify({ event: 'work_order_request_failed', request_id: metadata.requestId, code: /^[0-9A-Z]{5}$/.test(error?.code ?? '') ? error.code : 'INTERNAL' }))
      json(response, 500, { code: 'SERVER_ERROR', message: '业务服务暂时无法完成请求，请稍后重试' })
    })
  })
  server.requestTimeout = 15000
  server.headersTimeout = 10000
  server.keepAliveTimeout = 5000
  return server
}
