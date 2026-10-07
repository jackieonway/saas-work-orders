// Offline review artifacts only. This module neither connects to a server nor executes SQL.
const ERROR_CATEGORY = 'INVALID_GRANT_MANIFEST'
const PERMISSIONS = Object.freeze([
  'ticket:read', 'ticket:create', 'ticket:update', 'ticket:delete', 'ticket:export',
  'notification:publish',
])
const SCOPES = new Set(['SELF', 'ORG', 'ORG_SUBTREE', 'TENANT'])
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

function invalid() { throw new Error(ERROR_CATEGORY) }

function record(value, required, optional = []) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) invalid()
  const prototype = Object.getPrototypeOf(value)
  if (prototype !== Object.prototype && prototype !== null) invalid()
  const allowed = new Set([...required, ...optional])
  for (const key of Reflect.ownKeys(value)) {
    if (typeof key !== 'string' || !allowed.has(key)) invalid()
    const descriptor = Object.getOwnPropertyDescriptor(value, key)
    if (!descriptor || !Object.hasOwn(descriptor, 'value')) invalid()
  }
  if (required.some(key => !Object.hasOwn(value, key))) invalid()
}

function array(value) {
  if (!Array.isArray(value) || Object.getPrototypeOf(value) !== Array.prototype) invalid()
  const keys = Reflect.ownKeys(value)
  if (keys.length !== value.length + 1 || keys.some(key => key !== 'length' &&
    (typeof key !== 'string' || !/^(0|[1-9][0-9]*)$/.test(key) || Number(key) >= value.length))) invalid()
  for (let index = 0; index < value.length; index++) {
    const descriptor = Object.getOwnPropertyDescriptor(value, String(index))
    if (!descriptor || !Object.hasOwn(descriptor, 'value')) invalid()
  }
  return value
}

function uuid(value) {
  if (typeof value !== 'string' || !UUID.test(value)) invalid()
  return value.toLowerCase()
}

/** Validate and copy the complete manifest; omitted targets mean no authorization changes. */
export function validateGrantManifest(value) {
  try {
    record(value, ['schemaVersion', 'applicationCode', 'applicationId', 'menuIds'], ['targets'])
    if (value.schemaVersion !== '1.0' || value.applicationCode !== 'work-orders') invalid()
    const applicationId = uuid(value.applicationId)
    record(value.menuIds, ['root', ...PERMISSIONS])
    const menuIds = Object.fromEntries(['root', ...PERMISSIONS].map(key => [key, uuid(value.menuIds[key])]))
    if (new Set(Object.values(menuIds)).size !== Object.keys(menuIds).length) invalid()
    const members = new Set()
    const userOrganizations = new Set()
    const organizationTenants = new Map()
    const roleOrganizations = new Map()
    const targets = array(Object.hasOwn(value, 'targets') ? value.targets : []).map(target => {
      record(target, ['tenantId', 'organizationId', 'roleId', 'userId', 'membershipId', 'permissions', 'dataGrants'])
      const ids = Object.fromEntries(['tenantId', 'organizationId', 'roleId', 'userId', 'membershipId']
        .map(key => [key, uuid(target[key])]))
      const context = `${ids.userId}:${ids.organizationId}`
      if (members.has(ids.membershipId) || userOrganizations.has(context)) invalid()
      if (organizationTenants.has(ids.organizationId) && organizationTenants.get(ids.organizationId) !== ids.tenantId) invalid()
      if (roleOrganizations.has(ids.roleId) && roleOrganizations.get(ids.roleId) !== `${ids.tenantId}:${ids.organizationId}`) invalid()
      organizationTenants.set(ids.organizationId, ids.tenantId)
      roleOrganizations.set(ids.roleId, `${ids.tenantId}:${ids.organizationId}`)
      members.add(ids.membershipId)
      userOrganizations.add(context)
      const permissions = [...array(target.permissions)]
      if (permissions.some(permission => !PERMISSIONS.includes(permission)) ||
        new Set(permissions).size !== permissions.length) invalid()
      const actions = new Set()
      const dataGrants = array(target.dataGrants).map(grant => {
        record(grant, ['resource', 'action', 'scope'])
        const permission = `${grant.resource}:${grant.action}`
        if (!PERMISSIONS.includes(permission) ||
          !['ticket', 'notification'].includes(grant.resource) || !SCOPES.has(grant.scope)) invalid()
        if (actions.has(permission)) invalid()
        actions.add(permission)
        return { resource: grant.resource, action: grant.action, scope: grant.scope }
      })
      return { ...ids, permissions, dataGrants }
    })
    return { schemaVersion: '1.0', applicationCode: 'work-orders', applicationId, menuIds, targets }
  } catch {
    // Never include supplied values, property names or parser details in an error.
    throw new Error(ERROR_CATEGORY)
  }
}

function unionInto(map, key, values) {
  if (!map.has(key)) map.set(key, new Set())
  for (const value of values) map.get(key).add(value)
}

/** Human review plan. PUT bodies for replacing role sets are deliberately not precomputed. */
export function grantPlan(value) {
  const manifest = validateGrantManifest(value)
  const tenants = new Map()
  const organizations = new Map()
  const roles = new Map()
  for (const target of manifest.targets) {
    const selectedMenuIds = [manifest.menuIds.root, ...target.permissions.map(permission => manifest.menuIds[permission])]
    unionInto(tenants, target.tenantId, selectedMenuIds)
    unionInto(organizations, target.organizationId, selectedMenuIds)
    if (!roles.has(target.roleId)) roles.set(target.roleId, { menuIds: new Set(), userIds: new Set() })
    const role = roles.get(target.roleId)
    for (const menuId of selectedMenuIds) role.menuIds.add(menuId)
    role.userIds.add(target.userId)
  }
  const requests = [...[...tenants].map(([id, menus]) => ['tenants', id, menus]),
    ...[...organizations].map(([id, menus]) => ['organizations', id, menus])]
    .map(([kind, id, menus]) => ({
      method: 'PUT', path: `/api/${kind}/${id}/authorizations`, manualReviewRequired: true,
      body: { applications: [{ applicationId: manifest.applicationId, authorized: true }],
        menus: [...menus].map(menuId => ({ menuId, authorized: true })) },
    }))
  return {
    schemaVersion: '1.0', applicationCode: manifest.applicationCode, applicationId: manifest.applicationId,
    mode: 'MANUAL_REVIEW_ONLY', requests,
    roleMerges: [...roles].map(([roleId, role]) => ({
      roleId, getMenus: `/api/roles/${roleId}/menus`, getUsers: `/api/roles/${roleId}/users`,
      addMenuIds: [...role.menuIds], addUserIds: [...role.userIds],
      putMenus: `/api/roles/${roleId}/menus`, putUsers: `/api/roles/${roleId}/users`,
      instructions: [
        'Pause concurrent administration of this role and read both current sets immediately before writing.',
        'PUT menus with {menuIds: union(current menuIds, addMenuIds)}; this endpoint replaces the entire set.',
        'PUT users with {userIds: union(current response userId values, addUserIds)}; this endpoint replaces the entire set.',
        'Read both sets again and verify all existing values were preserved.',
        'Review all current role members: adding menus changes their permissions too; approve the complete audience before writing.',
      ],
    })),
    ownershipChecks: manifest.targets.map(({ permissions, dataGrants, ...ids }) => ({
      ...ids, applicationId: manifest.applicationId,
      requirements: [
        'The organization and role belong to the explicit tenant; the role belongs to the explicit organization.',
        'The membership ID already binds the explicit user directly to the explicit organization.',
        'Application, tenant, organization, user, role and selected menus are enabled; tenant and organization are unexpired.',
        'All menu IDs belong to work-orders and carry their exact declared permissions; the root path is /micro/work-orders.',
      ],
    })),
    dataGrants: manifest.targets.map(target => ({
      tenantId: target.tenantId, organizationId: target.organizationId, userId: target.userId,
      membershipId: target.membershipId, applicationId: manifest.applicationId, grants: target.dataGrants,
    })),
    warnings: [
      'This plan makes no HTTP requests and grants no permissions itself.',
      'Tenant and organization requests change only the listed work-orders items, but their consistency cleanup can remove existing inconsistent authorizations.',
      'Role menus and users PUT endpoints replace complete sets and have no optimistic concurrency check.',
      'A shared role gives every current and newly added member the union of its menus; use a dedicated reviewed role when permission audiences differ.',
      'Do not use the memberships PUT endpoint to add a member: it rebuilds memberships and deletes their independent data grants.',
      'Data grants have no public management write API; separately review the generated SQL with a DBA.',
      'Role permissions can be inherited by child organizations; independent data grants remain bound to the explicit direct membership.',
      'No operation permissions or data scopes are implied by one another; verify both in a real non-super user application context.',
    ],
  }
}

// Every interpolated value comes from UUID validation or a fixed, closed enumeration above.
function quotedUuid(value) { return `'${value}'::uuid` }
function idList(targets, key) { return [...new Set(targets.map(target => target[key]))].sort().map(quotedUuid).join(', ') }
function guard(condition) {
  return `  IF NOT (${condition}) THEN\n    RAISE EXCEPTION 'GRANT_PRECONDITION_FAILED';\n  END IF;`
}

/** Render a review-only transaction. The DBA must explicitly replace the final ROLLBACK. */
export function grantSql(value) {
  const manifest = validateGrantManifest(value)
  const header = '-- Review exact ownership and existing grants before execution.\n-- Default is ROLLBACK; a DBA may replace only the final statement after approval.\nBEGIN;\n'
  if (manifest.targets.length === 0) return `${header}-- No targets: no authorization or data-grant writes.\nROLLBACK;\n`
  const app = quotedUuid(manifest.applicationId)
  const lines = [header, 'DO $work_orders_grants$', 'BEGIN',
    `  PERFORM id FROM applications WHERE id = ${app} FOR KEY SHARE;`,
    guard('FOUND'),
    // KEY SHARE protects identity/deletion; SHARE additionally pins runtime, entry type and status.
    `  PERFORM id FROM applications WHERE id = ${app} FOR SHARE;`,
    guard(`EXISTS (SELECT 1 FROM applications WHERE id = ${app} AND code = 'work-orders' AND runtime_mode = 'IFRAME' AND entry_type = 'HTML' AND status = 'ENABLED' AND base_path = '/micro/work-orders')`),
  ]
  const menuEntries = Object.entries(manifest.menuIds).sort((left, right) => left[1].localeCompare(right[1]))
  for (const [permission, menuId] of menuEntries) {
    const menu = quotedUuid(menuId)
    lines.push(`  PERFORM id FROM menus WHERE id = ${menu} FOR SHARE;`, guard('FOUND'))
    const exact = permission === 'root'
      ? "COALESCE(permission, '') = '' AND path = '/micro/work-orders' AND type = 'MENU' AND parent_id IS NULL AND visible = true AND link_target = 'MICRO' AND link_url = ''"
      : `permission = '${permission}' AND type = 'BUTTON' AND parent_id = ${quotedUuid(manifest.menuIds.root)} AND path = '' AND visible = false AND link_target = 'BLANK' AND link_url = ''`
    lines.push(guard(`EXISTS (SELECT 1 FROM menus WHERE id = ${menu} AND application_id = ${app} AND status = 'ENABLED' AND ${exact})`))
  }
  // Acquire shared owners in a deterministic order before inspecting their mutable attributes.
  for (const [table, key] of [['tenants', 'tenantId'], ['organizations', 'organizationId'], ['users', 'userId'], ['roles', 'roleId']]) {
    lines.push(`  PERFORM id FROM ${table} WHERE id IN (${idList(manifest.targets, key)}) ORDER BY id FOR SHARE;`)
  }
  lines.push(`  PERFORM id FROM memberships WHERE id IN (${idList(manifest.targets, 'membershipId')}) ORDER BY id FOR UPDATE;`)
  for (const target of manifest.targets) {
    const tenant = quotedUuid(target.tenantId)
    const org = quotedUuid(target.organizationId)
    const user = quotedUuid(target.userId)
    const role = quotedUuid(target.roleId)
    const member = quotedUuid(target.membershipId)
    lines.push(guard(`EXISTS (SELECT 1 FROM memberships member JOIN users u ON u.id = member.user_id JOIN organizations o ON o.id = member.organization_id JOIN tenants t ON t.id = o.tenant_id JOIN roles r ON r.id = ${role} AND r.organization_id = o.id AND r.tenant_id = t.id WHERE member.id = ${member} AND member.user_id = ${user} AND member.organization_id = ${org} AND t.id = ${tenant} AND u.status = 'ENABLED' AND o.status = 'ENABLED' AND t.status = 'ENABLED' AND r.status = 'ENABLED' AND (o.expires_at IS NULL OR o.expires_at > clock_timestamp()) AND (t.expires_at IS NULL OR t.expires_at > clock_timestamp()))`))
  }
  // Check existing scopes before any INSERT, then check again after ON CONFLICT in case another writer raced.
  for (const target of manifest.targets) {
    for (const grant of target.dataGrants) {
      const key = `membership_id = ${quotedUuid(target.membershipId)} AND application_id = ${app} AND resource_code = '${grant.resource}' AND action_code = '${grant.action}'`
      lines.push(`  PERFORM id FROM membership_data_grants WHERE ${key} FOR UPDATE;`,
        guard(`NOT EXISTS (SELECT 1 FROM membership_data_grants WHERE ${key} AND scope_type <> '${grant.scope}')`))
    }
  }
  for (const target of manifest.targets) {
    for (const grant of target.dataGrants) {
      const member = quotedUuid(target.membershipId)
      const key = `membership_id = ${member} AND application_id = ${app} AND resource_code = '${grant.resource}' AND action_code = '${grant.action}'`
      lines.push(
        `  INSERT INTO membership_data_grants (membership_id, application_id, resource_code, action_code, scope_type) VALUES (${member}, ${app}, '${grant.resource}', '${grant.action}', '${grant.scope}') ON CONFLICT (membership_id, application_id, resource_code, action_code) DO NOTHING;`,
        `  PERFORM id FROM membership_data_grants WHERE ${key} FOR UPDATE;`,
        guard(`EXISTS (SELECT 1 FROM membership_data_grants WHERE ${key} AND scope_type = '${grant.scope}')`),
      )
    }
  }
  lines.push('END;', '$work_orders_grants$;', 'ROLLBACK;', '')
  return lines.join('\n')
}
