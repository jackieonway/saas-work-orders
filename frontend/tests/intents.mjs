import assert from 'node:assert/strict'
import test from 'node:test'
import { IntentStore, identityKey } from '../src/intents.mjs'

const context = (extra = {}) => ({ contractVersion: '2.0', appCode: 'work-orders', basePath: '/micro/work-orders',
  contextVersion: 'one', contextEpoch: 1, token: 'must-not-be-stored', expiresAt: new Date(Date.now() + 60_000).toISOString(),
  tokenExpiresAt: new Date(Date.now() + 60_000).toISOString(), signal: new AbortController().signal,
  tenant: { id: 'tenant' }, organization: { id: 'organization' }, user: { id: 'user' },
  permissions: ['ticket:create'], dataGrants: [], ...extra })
const store = () => new IntentStore({ randomUUID: () => '12345678-1234-1234-1234-123456789abc' })

test('creation captures immutable exact DTO and key before transport, and retains both until explicit settlement', () => {
  const vault = store(), props = context(), body = { title: ' Original ', description: 'details', priority: 'HIGH', assigneeUserId: null }
  const intent = vault.prepareCreate(props, body)
  body.title = 'changed after dispatch'
  assert.equal(intent.key, 'create:12345678-1234-1234-1234-123456789abc')
  assert.deepEqual(intent.body, { title: ' Original ', description: 'details', priority: 'HIGH', assigneeUserId: null })
  assert.ok(Object.isFrozen(intent)); assert.ok(Object.isFrozen(intent.body))
  assert.equal(vault.prepareCreate(props, { title: 'different event' }), intent)
  assert.equal(vault.getCreate(props), intent)
  assert.equal(vault.settleCreate(props, { ...intent }), false)
  assert.equal(vault.settleCreate(props, intent), true)
  assert.equal(vault.getCreate(props), null)
})

test('intent vault isolates app, tenant, organization and user while normal renewal preserves legal pending intents', () => {
  const vault = store(), props = context(), original = vault.prepareCreate(props, { title: 'original' })
  const renewed = { ...props, contextEpoch: 2, contextVersion: 'two', token: 'renewed', signal: new AbortController().signal }
  assert.equal(identityKey(props), identityKey(renewed)); assert.equal(vault.getCreate(renewed), original)
  for (const changed of [{ tenant: { id: 'other' } }, { organization: { id: 'other' } }, { user: { id: 'other' } }]) {
    const next = { ...renewed, ...changed }
    assert.equal(vault.getCreate(next), null)
    assert.equal(vault.settleCreate(next, original), false)
    assert.notEqual(vault.prepareCreate(next, { title: 'separate' }), original)
  }
  assert.equal(vault.getCreate({ ...renewed, appCode: 'another-app' }), null)
  assert.equal(vault.getCreate(renewed), original)
  assert.equal(JSON.stringify(original).includes('must-not-be-stored'), false)
})

test('reminders retain independent per-ticket immutable events and never include recipient or credentials', () => {
  let sequence = 0
  const vault = new IntentStore({ randomUUID: () => 'event-' + ++sequence }), props = context()
  const one = vault.prepareReminder(props, 'ticket-a'), two = vault.prepareReminder(props, 'ticket-b')
  assert.notEqual(one.key, two.key)
  assert.deepEqual(one, { kind: 'reminder', key: 'reminder:event-1', ticketId: 'ticket-a', body: { idempotencyKey: 'reminder:event-1' } })
  assert.ok(Object.isFrozen(one.body)); assert.equal(vault.prepareReminder(props, 'ticket-a'), one)
  assert.equal(vault.settleReminder({ ...props, user: { id: 'another' } }, one), false)
  assert.equal(vault.settleReminder(props, one), true)
  assert.equal(vault.getReminder(props, 'ticket-a'), null); assert.equal(vault.getReminder(props, 'ticket-b'), two)
})

test('invalid contexts and DTOs cannot create pending events or leak arbitrary fields into the vault', () => {
  const vault = store(), props = context(), aborted = new AbortController(); aborted.abort()
  for (const invalid of [{ ...props, signal: aborted.signal }, { ...props, token: '' }, { ...props, expiresAt: 'invalid' }]) {
    assert.equal(identityKey(invalid), null)
    assert.throws(() => vault.prepareCreate(invalid, { title: 'title' }), error => error.name === 'AbortError')
    assert.equal(vault.getCreate(invalid), null)
  }
  for (const body of [null, [], { title: 'title', token: 'secret' }, { title: 1 }, { title: 'title', assigneeUserId: {} }, { title: 'title', priority: 'constructor' }]) {
    assert.throws(() => vault.prepareCreate(props, body), TypeError)
    assert.equal(vault.getCreate(props), null)
  }
  assert.throws(() => vault.prepareReminder(props, ''), TypeError)
})
