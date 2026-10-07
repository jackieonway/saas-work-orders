import assert from 'node:assert/strict'
import test from 'node:test'
import { grantPlan, grantSql, validateGrantManifest } from '../grants.mjs'

const id = number => `00000000-0000-4000-8000-${number.toString(16).padStart(12, '0')}`
function manifest() {
  return {
    schemaVersion: '1.0', applicationCode: 'work-orders', applicationId: id(1),
    menuIds: { root: id(2), 'ticket:read': id(3), 'ticket:create': id(4), 'ticket:update': id(5),
      'ticket:delete': id(6), 'ticket:export': id(7), 'notification:publish': id(8) },
    targets: [{ tenantId: id(9), organizationId: id(10), roleId: id(11), userId: id(12),
      membershipId: id(13), permissions: ['ticket:read', 'notification:publish'],
      dataGrants: [{ resource: 'ticket', action: 'read', scope: 'SELF' },
        { resource: 'notification', action: 'publish', scope: 'ORG' }] }],
  }
}
function rejected(value) {
  for (const api of [validateGrantManifest, grantPlan, grantSql]) {
    assert.throws(() => api(value), error => error.constructor === Error && error.message === 'INVALID_GRANT_MANIFEST')
  }
}

test('validated manifests are independent canonical copies with no implied actions', () => {
  const value = manifest()
  value.applicationId = 'AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA'
  const validated = validateGrantManifest(value)
  assert.equal(validated.applicationId, value.applicationId.toLowerCase())
  assert.deepEqual(validated.targets[0].permissions, ['ticket:read', 'notification:publish'])
  validated.targets[0].dataGrants[0].scope = 'TENANT'
  assert.equal(value.targets[0].dataGrants[0].scope, 'SELF')
})

test('omitted or empty targets produce no requests or SQL writes', () => {
  for (const omitted of [true, false]) {
    const value = manifest()
    if (omitted) delete value.targets
    else value.targets = []
    assert.deepEqual(validateGrantManifest(value).targets, [])
    const plan = grantPlan(value)
    for (const key of ['requests', 'roleMerges', 'ownershipChecks', 'dataGrants']) assert.deepEqual(plan[key], [])
    const sql = grantSql(value)
    assert.match(sql, /BEGIN;/)
    assert.match(sql, /ROLLBACK;\s*$/)
    assert.doesNotMatch(sql, /\b(?:INSERT|UPDATE|DELETE)\b/)
  }
})

test('unknown fields at every level and secret values never escape validation errors', () => {
  for (const location of ['manifest', 'menus', 'target', 'grant']) {
    const value = manifest()
    const target = location === 'manifest' ? value : location === 'menus' ? value.menuIds :
      location === 'target' ? value.targets[0] : value.targets[0].dataGrants[0]
    target.clientSecret = 'do-not-expose-this-secret'
    rejected(value)
  }
})

test('manifest schema, mandatory keys and collection types are strict', () => {
  const edits = [
    value => { value.schemaVersion = '2.0' }, value => { value.applicationCode = 'other-app' },
    value => { delete value.menuIds.root }, value => { delete value.targets[0].permissions },
    value => { delete value.targets[0].dataGrants }, value => { value.targets = null },
    value => { value.targets = {} }, value => { value.targets[0].permissions = 'ticket:read' },
    value => { value.targets[0].dataGrants = {} }, value => { value.targets[0].permissions.extra = 'secret' },
    value => { value.targets = new Array(1) }, value => { value.menuIds = Object.create({ ...value.menuIds }) },
  ]
  for (const edit of edits) { const value = manifest(); edit(value); rejected(value) }
  for (const value of [null, [], 'secret', 1]) rejected(value)
})

test('UUID fields reject injection, whitespace, nonstrings and duplicate menus after normalization', () => {
  for (const invalid of ["00000000-0000-4000-8000-000000000001'); DELETE FROM users; --", ' not-a-uuid ', null, 123]) {
    for (const key of ['tenantId', 'organizationId', 'roleId', 'userId', 'membershipId']) {
      const value = manifest(); value.targets[0][key] = invalid; rejected(value)
    }
    const value = manifest(); value.applicationId = invalid; rejected(value)
  }
  const duplicate = manifest()
  duplicate.menuIds.root = 'AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA'
  duplicate.menuIds['ticket:read'] = duplicate.menuIds.root.toLowerCase()
  rejected(duplicate)
})

test('permissions, resource/action pairs and scopes use closed approved enumerations', () => {
  const badGrants = [
    { resource: 'ticket', action: 'publish', scope: 'SELF' },
    { resource: 'notification', action: 'read', scope: 'ORG' },
    { resource: 'other', action: 'read', scope: 'SELF' },
    { resource: 'ticket', action: 'read', scope: 'ALL' },
    { resource: 'ticket', action: 'read', scope: 'self' },
    { resource: 'ticket', action: "read'; COMMIT; --", scope: 'SELF' },
  ]
  for (const grant of badGrants) { const value = manifest(); value.targets[0].dataGrants = [grant]; rejected(value) }
  for (const permissions of [['system:user'], ['ticket:read', 'ticket:read'], ['root'], [null]]) {
    const value = manifest(); value.targets[0].permissions = permissions; rejected(value)
  }
  for (const scope of ['SELF', 'ORG', 'ORG_SUBTREE', 'TENANT']) {
    const value = manifest(); value.targets[0].dataGrants[0].scope = scope
    assert.equal(validateGrantManifest(value).targets[0].dataGrants[0].scope, scope)
  }
})

test('duplicate membership, duplicate direct context and duplicate/conflicting action scopes are rejected', () => {
  const sameMember = manifest()
  sameMember.targets.push({ ...structuredClone(sameMember.targets[0]), userId: id(19), organizationId: id(20) })
  rejected(sameMember)
  const sameContext = manifest()
  sameContext.targets.push({ ...structuredClone(sameContext.targets[0]), membershipId: id(19) })
  rejected(sameContext)
  for (const scope of ['SELF', 'TENANT']) {
    const value = manifest()
    value.targets[0].dataGrants.push({ resource: 'ticket', action: 'read', scope })
    rejected(value)
  }
})

test('a shared organization or role cannot claim inconsistent tenant or organization ownership', () => {
  for (const change of [
    target => { target.tenantId = id(30); target.roleId = id(31) },
    target => { target.organizationId = id(30) },
  ]) {
    const value = manifest()
    const target = { ...structuredClone(value.targets[0]), userId: id(14), membershipId: id(15) }
    change(target)
    value.targets.push(target)
    rejected(value)
  }
})

test('manual plan changes only the work orders app and declared menus, preserving role sets by instructions', () => {
  const value = manifest()
  const plan = grantPlan(value)
  assert.equal(plan.mode, 'MANUAL_REVIEW_ONLY')
  assert.deepEqual(plan.requests.map(request => request.path), [
    `/api/tenants/${id(9)}/authorizations`, `/api/organizations/${id(10)}/authorizations`,
  ])
  for (const request of plan.requests) {
    assert.equal(request.manualReviewRequired, true)
    assert.deepEqual(request.body.applications, [{ applicationId: id(1), authorized: true }])
    assert.deepEqual(request.body.menus, [id(2), id(3), id(8)].map(menuId => ({ menuId, authorized: true })))
  }
  assert.deepEqual(plan.roleMerges[0].addMenuIds, [id(2), id(3), id(8)])
  assert.deepEqual(plan.roleMerges[0].addUserIds, [id(12)])
  assert.equal(Object.hasOwn(plan.roleMerges[0], 'body'), false)
  assert.match(plan.roleMerges[0].instructions.join(' '), /union\(current menuIds, addMenuIds\)/)
  assert.match(plan.warnings.join(' '), /memberships PUT/)
  assert.deepEqual(plan.dataGrants[0].grants, value.targets[0].dataGrants)
})

test('multiple explicit members share app/menu requests and union role additions without duplicate target writes', () => {
  const value = manifest()
  value.targets.push({ ...structuredClone(value.targets[0]), userId: id(14), membershipId: id(15),
    permissions: ['ticket:create'], dataGrants: [] })
  const plan = grantPlan(value)
  assert.equal(plan.requests.length, 2)
  assert.equal(plan.roleMerges.length, 1)
  assert.deepEqual(plan.roleMerges[0].addUserIds, [id(12), id(14)])
  assert.deepEqual(plan.roleMerges[0].addMenuIds, [id(2), id(3), id(8), id(4)])
  assert.equal(plan.ownershipChecks.length, 2)
  assert.equal((grantSql(value).match(/INSERT INTO membership_data_grants/g) || []).length, 2)
})

test('SQL locks owners before guards and writes only exact approved grants, with a rollback default', () => {
  const sql = grantSql(manifest())
  assert.match(sql, /BEGIN;\s+DO \$work_orders_grants\$/)
  assert.match(sql, /FOR KEY SHARE/)
  assert.match(sql, /runtime_mode = 'IFRAME' AND entry_type = 'HTML' AND status = 'ENABLED'/)
  assert.match(sql, /path = '\/micro\/work-orders'/)
  assert.match(sql, /base_path = '\/micro\/work-orders'/)
  assert.match(sql, /parent_id IS NULL AND visible = true AND link_target = 'MICRO' AND link_url = ''/)
  assert.match(sql, /permission = 'ticket:read' AND type = 'BUTTON' AND parent_id = '00000000-0000-4000-8000-000000000002'::uuid AND path = '' AND visible = false AND link_target = 'BLANK' AND link_url = ''/)
  assert.match(sql, /member\.id = '00000000-0000-4000-8000-00000000000d'::uuid/)
  assert.match(sql, /r\.organization_id = o\.id AND r\.tenant_id = t\.id/)
  assert.match(sql, /o\.expires_at > clock_timestamp\(\)/)
  assert.match(sql, /t\.expires_at > clock_timestamp\(\)/)
  const body = sql.slice(sql.indexOf('DO $work_orders_grants$'))
  assert.doesNotMatch(body, /\bDELETE\b|\bDO UPDATE\b|\bCOMMIT\b/)
  assert.deepEqual([...body.matchAll(/INSERT INTO ([a-z_]+)/g)].map(match => match[1]),
    ['membership_data_grants', 'membership_data_grants'])
  assert.match(body, /ON CONFLICT \(membership_id, application_id, resource_code, action_code\) DO NOTHING/g)
  assert.match(body, /scope_type <> 'SELF'/)
  assert.match(body, /scope_type = 'SELF'/)
  assert.match(body, /scope_type <> 'ORG'/)
  assert.doesNotMatch(body, /'create'|'update'|'delete'|'export'|'TENANT'/)
  assert.ok(body.indexOf('FROM memberships WHERE') < body.indexOf('JOIN users u'))
  assert.ok(body.indexOf("scope_type <> 'ORG'") < body.indexOf('INSERT INTO'))
  assert.match(sql, /ROLLBACK;\s*$/)
})

test('targets with no data grants never acquire an implied independent grant', () => {
  const value = manifest()
  value.targets[0].permissions = []
  value.targets[0].dataGrants = []
  assert.doesNotMatch(grantSql(value), /INSERT INTO/)
  assert.deepEqual(grantPlan(value).dataGrants[0].grants, [])
  assert.deepEqual(grantPlan(value).requests[0].body.menus, [{ menuId: id(2), authorized: true }])
})
