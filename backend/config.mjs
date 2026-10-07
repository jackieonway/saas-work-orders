import { metricsToken } from './observability.mjs'

const LOOPBACK = new Set(['127.0.0.1', 'localhost', '[::1]'])

function integer(env, name, fallback, min, max) {
  const raw = env[name] ?? String(fallback)
  if (!/^\d+$/.test(raw) || Number(raw) < min || Number(raw) > max) {
    throw new Error(`${name} must be an integer between ${min} and ${max}`)
  }
  return Number(raw)
}

function origin(value, production, name) {
  try {
    const url = new URL(value)
    if (url.origin !== value || url.username || url.password || url.search || url.hash ||
        (url.protocol !== 'https:' && !(url.protocol === 'http:' && !production && LOOPBACK.has(url.hostname)))) throw new Error()
    return url.origin
  } catch { throw new Error(`${name} must be an exact HTTPS origin (development permits loopback HTTP)` ) }
}

export function loadConfig(env = process.env) {
  const production = env.NODE_ENV === 'production'
  if (production && env.NODE_TLS_REJECT_UNAUTHORIZED === '0') throw new Error('NODE_TLS_REJECT_UNAUTHORIZED cannot disable production TLS verification')
  const databaseUrl = env.WORK_ORDERS_DATABASE_URL?.trim()
  try {
    if (!databaseUrl || !['postgres:', 'postgresql:'].includes(new URL(databaseUrl).protocol)) throw new Error()
  } catch { throw new Error('WORK_ORDERS_DATABASE_URL must be the independent business PostgreSQL URL') }
  const platformOrigin = origin(env.PLATFORM_API_ORIGIN?.trim(), production, 'PLATFORM_API_ORIGIN')
  const clientId = env.PLATFORM_CLIENT_ID?.trim()
  const clientSecret = env.PLATFORM_CLIENT_SECRET
  if (!clientId || !/^[A-Za-z0-9._-]{1,100}$/.test(clientId)) throw new Error('PLATFORM_CLIENT_ID is required')
  if (!clientSecret || clientSecret.length > 1024 || /[\r\n]/.test(clientSecret)) throw new Error('PLATFORM_CLIENT_SECRET is required')
  const host = env.WORK_ORDERS_HOST?.trim() || '0.0.0.0'
  if (!host || /[\s/\\]/.test(host)) throw new Error('WORK_ORDERS_HOST must be a bind address')
  const allowedOrigins = [...new Set((env.WORK_ORDERS_ALLOWED_ORIGINS ?? 'null').split(',').map((value) => {
    const item = value.trim()
    return item === 'null' ? item : origin(item, production, 'WORK_ORDERS_ALLOWED_ORIGINS')
  }))]
  return Object.freeze({
    databaseUrl, platformOrigin, clientId, clientSecret, appCode: 'work-orders', host, production,
    metricsToken: metricsToken(env, production),
    port: integer(env, 'WORK_ORDERS_PORT', 3004, production ? 1 : 0, 65535),
    platformTimeoutMs: integer(env, 'PLATFORM_AUTH_TIMEOUT_MS', 3000, 100, 10000),
    maxPlatformBodyBytes: 256 * 1024,
    maxBodyBytes: 64 * 1024,
    allowedOrigins: Object.freeze(allowedOrigins),
  })
}
