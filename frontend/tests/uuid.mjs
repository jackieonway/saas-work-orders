import assert from 'node:assert/strict'
import test from 'node:test'
import { newId, validUUID } from '../src/uuid.mjs'
import { canTicket, validContext } from '../src/domain.mjs'
import { WorkOrdersClient } from '../src/client.mjs'

const invalidIds = [
  '00000000-0000-0000-0000-000000000000',
  '0199fe00-0000-4000-8000-000000000001', // explicit legacy UUIDv4 negative
  '0199fe00-0000-7000-0000-000000000001',
  '0199fe00-0000-7000-c000-000000000001',
  '0199fe00-0000-7000-f000-000000000001',
]
function context() {
  return { contractVersion: '2.0', appCode: 'work-orders', basePath: '/micro/work-orders',
    contextVersion: 'uuid-boundary', contextEpoch: 1, token: 'fixture-only-token',
    signal: new AbortController().signal, expiresAt: new Date(Date.now() + 60000).toISOString(),
    tokenExpiresAt: new Date(Date.now() + 60000).toISOString(),
    user: { id: newId() }, tenant: { id: newId() }, organization: { id: newId() },
    permissions: [], dataGrants: [] }
}

test('typed identity and resource inputs reject legacy, nil and non-RFC UUID variants before transport', async () => {
  const props = context()
  let sent = 0
  const client = new WorkOrdersClient({ fetchImpl: async () => { sent++; throw Error('unexpected transport') } })
  client.update(props)
  assert.equal(validUUID(newId()), true)
  for (const id of invalidIds) {
    assert.equal(validUUID(id), false)
    for (const kind of ['user', 'tenant', 'organization']) assert.equal(validContext({ ...props, [kind]: { id } }), false)
    assert.equal(validContext({ ...props, organizationScopeIds: [props.organization.id, id] }), false)
    const granted = { ...props, permissions: ['ticket:read'], organizationScopeIds: null, dataGrants: [{
      resource: 'ticket', action: 'read', scope: 'TENANT', subjectUserId: props.user.id, tenantId: props.tenant.id, organizationIds: [] }] }
    const row = { id: newId(), tenantId: props.tenant.id, organizationId: props.organization.id, ownerUserId: props.user.id }
    for (const field of ['id', 'organizationId', 'ownerUserId', 'assigneeUserId']) assert.equal(canTicket(granted, 'read', { ...row, [field]: id }), false)
    for (const operation of [() => client.detail(id), () => client.updateTicket(id, { version: 1 }),
      () => client.remove(id, 1), () => client.remind(id, 'event'), () => client.assignees('create', id),
      () => client.create({ title: 'fixture', assigneeUserId: id }, 'event')]) await assert.rejects(operation, TypeError)
  }
  assert.equal(sent, 0)
  client.stop()
})

test('transport emits UUIDv7 even with the frozen SDK and rejects an echoed legacy request ID', async () => {
  let sent
  const client = new WorkOrdersClient({ fetchImpl: async (_, options) => {
    sent = options.headers.get('X-Request-Id')
    return new Response('{}', { status: 503, headers: { 'Content-Type': 'application/json', 'X-Request-Id': invalidIds[1] } })
  } })
  client.update(context())
  await assert.rejects(client.list({}), error => validUUID(error.requestId) && error.requestId === sent)
  assert.equal(validUUID(sent), true)
  client.stop()
})
