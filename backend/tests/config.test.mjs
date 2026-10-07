import assert from 'node:assert/strict'
import test from 'node:test'
import { loadConfig } from '../config.mjs'

const fixtureEnv = (patch = {}) => ({
  WORK_ORDERS_DATABASE_URL: 'postgresql://fixture-only@127.0.0.1/fixture_never_connected',
  PLATFORM_API_ORIGIN: 'https://platform.example.test', PLATFORM_CLIENT_ID: 'work-orders-server',
  PLATFORM_CLIENT_SECRET: 'config-fixture-only-secret',
  METRICS_TOKEN: 'FixtureOnlyMetricsToken0123456789AbCd',
  ...patch,
})

test('work-order config accepts exact HTTPS and explicit development loopback HTTP only', () => {
  const production = loadConfig(fixtureEnv({ NODE_ENV: 'production' }))
  assert.equal(production.platformOrigin, 'https://platform.example.test')
  assert.equal(production.appCode, 'work-orders')
  assert.equal(production.platformTimeoutMs, 3000)
  assert.equal(production.maxBodyBytes, 65536)
  assert.equal(production.maxPlatformBodyBytes, 262144)
  for (const origin of ['http://127.0.0.1:3456', 'http://localhost:3456', 'http://[::1]:3456']) {
    assert.equal(loadConfig(fixtureEnv({ PLATFORM_API_ORIGIN: origin })).platformOrigin, origin)
    assert.throws(() => loadConfig(fixtureEnv({ NODE_ENV: 'production', PLATFORM_API_ORIGIN: origin })), /HTTPS origin/)
  }
  for (const origin of [
    'http://outside.example.test', 'http://127.0.0.2:3456', 'http://localhost.:3456',
    'https://platform.example.test/', 'https://platform.example.test/path', 'https://platform.example.test?query=1',
    'https://platform.example.test#fragment', 'https://fixture:password@platform.example.test',
    'file:///fixture', '', 'not-a-url',
  ]) assert.throws(() => loadConfig(fixtureEnv({ PLATFORM_API_ORIGIN: origin })), /HTTPS origin/)
})

test('work-order config refuses missing credentials, unsafe bind values and invalid timeouts', () => {
  for (const patch of [
    { WORK_ORDERS_DATABASE_URL: '' }, { WORK_ORDERS_DATABASE_URL: 'https://not-postgres.example.test' },
    { PLATFORM_CLIENT_ID: '' }, { PLATFORM_CLIENT_ID: 'invalid client' },
    { PLATFORM_CLIENT_SECRET: '' }, { PLATFORM_CLIENT_SECRET: 'secret\r\nheader' },
    { PLATFORM_AUTH_TIMEOUT_MS: '99' }, { PLATFORM_AUTH_TIMEOUT_MS: '10001' }, { PLATFORM_AUTH_TIMEOUT_MS: 'NaN' },
    { WORK_ORDERS_HOST: 'unsafe/host' }, { WORK_ORDERS_PORT: '65536' }, { NODE_ENV: 'production', WORK_ORDERS_PORT: '0' },
    { NODE_ENV: 'production', WORK_ORDERS_ALLOWED_ORIGINS: 'http://outside.example.test' },
  ]) assert.throws(() => loadConfig(fixtureEnv(patch)))
  const safe = loadConfig(fixtureEnv({ WORK_ORDERS_PORT: '0', WORK_ORDERS_ALLOWED_ORIGINS: 'null,https://host.example.test,null' }))
  assert.equal(safe.port, 0)
  assert.deepEqual(safe.allowedOrigins, ['null', 'https://host.example.test'])
})

test('production metrics require a strong independent token; development absent metrics remain closed', () => {
  assert.throws(() => loadConfig({ ...fixtureEnv(), NODE_ENV: 'production', METRICS_TOKEN: undefined }), /METRICS_TOKEN/)
  for (const token of ['', 'short', 'a'.repeat(40), 'A'.repeat(40), '1'.repeat(40), 'ValidLookingSecretWithDigits12345\n']) {
    assert.throws(() => loadConfig({ ...fixtureEnv(), NODE_ENV: 'production', METRICS_TOKEN: token }), /METRICS_TOKEN/)
  }
  assert.equal(loadConfig({ ...fixtureEnv(), METRICS_TOKEN: undefined }).metricsToken, null)
})
