import assert from 'node:assert/strict'
import test from 'node:test'
import { randomUUID } from 'node:crypto'
import {
  ACTIONS, API, accepted, call, createTicket, denied,
  grantFor, matrix, records, replaceGrant, testDatabaseUrl,
} from './helpers.mjs'

const databaseUrl = await testDatabaseUrl()
const options = { skip: databaseUrl ? false : 'Set TEST_DATABASE_URL or configure ignored root .env.test for real PostgreSQL acceptance', timeout: 180_000 }
const sortedIds = (rows) => rows.map((row) => row.id).sort()
const scopedKeys = {
  SELF: ['ownRoot', 'ownChild'],
  ORG: ['ownRoot', 'otherRoot'],
  ORG_SUBTREE: ['ownRoot', 'otherRoot', 'ownChild', 'otherChild'],
  TENANT: ['ownRoot', 'otherRoot', 'ownChild', 'otherChild', 'peer'],
}

test('real PG + HTTP platform fixture: four scopes isolate list/detail/export and refresh every request', options, async (t) => {
  const h = await matrix(t, databaseUrl)
  const actor = h.actors.root
  const path = (ticket) => `${API}/${ticket.id}`
  for (const [scope, keys] of Object.entries(scopedKeys)) {
    await t.test(`${scope} list/detail/CSV are tenant-bound and use explicit record ownership`, async () => {
      const orgs = scope === 'ORG' ? [h.root] : scope === 'ORG_SUBTREE' ? [h.root, h.child] : []
      replaceGrant(actor.state, 'read', scope, orgs)
      replaceGrant(actor.state, 'export', scope, orgs)
      const expected = keys.map((key) => h.tickets[key].id).sort()
      assert.deepEqual(sortedIds(await records(h.url, actor)), expected)
      for (const [key, ticket] of Object.entries(h.tickets)) {
        if (keys.includes(key)) {
          const row = await accepted(h.url, actor.token, 'GET', path(ticket))
          assert.equal(row.id, ticket.id)
          assert.equal(row.tenantId, h.tenantA)
          assert.equal(row.organizationId, ticket.organizationId)
          assert.equal(row.ownerUserId, ticket.ownerUserId)
        } else {
          const response = await denied(h.url, actor.token, 'GET', path(ticket), undefined, [404])
          assert.ok(!response.text.includes(ticket.title), 'hidden record title leaked through error')
          assert.ok(!response.text.includes(ticket.id), 'hidden record identifier leaked through error')
        }
      }
      const csv = await call(h.url, actor.token, 'GET', `${API}/export.csv`)
      assert.equal(csv.status, 200)
      assert.match(csv.headers.get('content-type'), /text\/csv/i)
      for (const [key, ticket] of Object.entries(h.tickets)) {
        assert.equal(csv.text.includes(ticket.title), keys.includes(key), `CSV leaked or omitted ${key} under ${scope}`)
      }
    })
  }

  await t.test('same token sees direct grant revocation and narrowing with no positive cache', async () => {
    replaceGrant(actor.state, 'read', 'TENANT')
    await accepted(h.url, actor.token, 'GET', path(h.tickets.peer))
    replaceGrant(actor.state, 'read', null)
    await denied(h.url, actor.token, 'GET', API, undefined, [403])
    replaceGrant(actor.state, 'read', 'SELF')
    await denied(h.url, actor.token, 'GET', path(h.tickets.otherRoot), undefined, [404])
    assert.deepEqual(sortedIds(await records(h.url, actor)), scopedKeys.SELF.map((key) => h.tickets[key].id).sort())
    assert.ok(actor.state.calls.introspect >= 20 && actor.state.calls.context >= 20, 'protected requests did not each re-contact the HTTP platform fixture')
  })

  await t.test('organization switch uses the new token and does not carry old ORG grant state', async () => {
    replaceGrant(actor.state, 'read', 'ORG', [h.root])
    replaceGrant(h.actors.ownChild.state, 'read', 'ORG', [h.child])
    assert.deepEqual(sortedIds(await records(h.url, actor)), [h.tickets.ownRoot.id, h.tickets.otherRoot.id].sort())
    assert.deepEqual(sortedIds(await records(h.url, h.actors.ownChild)), [h.tickets.ownChild.id, h.tickets.otherChild.id].sort())
    actor.state.active = false
    await denied(h.url, actor.token, 'GET', API, undefined, [401])
    await accepted(h.url, h.actors.ownChild.token, 'GET', API)
    actor.state.active = true
  })

  await t.test('operation grants and data grants are independent; hidden buttons cannot authorize API calls', async () => {
    replaceGrant(actor.state, 'update', 'TENANT')
    const original = actor.state.permissions
    actor.state.permissions = original.filter((permission) => permission !== 'ticket:update')
    await denied(h.url, actor.token, 'PATCH', path(h.tickets.ownRoot), { version: 1, title: 'Tampered hidden update button' }, [403])
    actor.state.permissions = original
    replaceGrant(actor.state, 'update', null)
    await denied(h.url, actor.token, 'PATCH', path(h.tickets.ownRoot), { version: 1, title: 'Operation role without data grant' }, [403])
    const row = await accepted(h.url, h.actors.other.token, 'GET', path(h.tickets.ownRoot))
    assert.equal(row.title, 'Own root ticket')
    assert.equal(row.version, 1)
  })

  await t.test('tenant query overrides never broaden list and filters operate on persistent records', async () => {
    replaceGrant(actor.state, 'read', 'TENANT')
    const tampered = await call(h.url, actor.token, 'GET', `${API}?tenantId=${h.tenantB}&organizationId=${h.remote}`)
    assert.ok([200, 400].includes(tampered.status))
    if (tampered.status === 200) assert.ok(tampered.value.records.every((row) => row.tenantId === h.tenantA))
    await createTicket(h.url, actor, 'High priority filter target', { priority: 'HIGH' })
    const high = await records(h.url, actor, '&priority=HIGH&status=OPEN')
    assert.equal(high.length, 1)
    assert.equal(high[0].title, 'High priority filter target')
    assert.ok(high.every((row) => row.priority === 'HIGH' && row.status === 'OPEN'))
    await denied(h.url, actor.token, 'GET', `${API}?priority=INVALID`, undefined, [400])
    await denied(h.url, actor.token, 'GET', `${API}?status=INVALID`, undefined, [400])
  })

  await t.test('all create scopes bind server ownership to the current organization', async () => {
    for (const scope of Object.keys(scopedKeys)) {
      replaceGrant(actor.state, 'create', scope, scope === 'ORG' ? [h.root] : scope === 'ORG_SUBTREE' ? [h.root, h.child] : [])
      const row = await createTicket(h.url, actor, `Creation scope ${scope}`)
      assert.equal(row.tenantId, h.tenantA)
      assert.equal(row.organizationId, h.root)
      assert.equal(row.ownerUserId, actor.actor.userId)
    }
    replaceGrant(actor.state, 'create', null)
    await denied(h.url, actor.token, 'POST', API, { title: 'No create data grant' }, [403])
  })
})

test('real PG + HTTP platform fixture: writes obey scopes, optimistic concurrency and soft deletion', options, async (t) => {
  const h = await matrix(t, databaseUrl)
  const actor = h.actors.root
  const path = (ticket) => `${API}/${ticket.id}`
  for (const [scope, keys] of Object.entries(scopedKeys)) {
    await t.test(`${scope} update/delete cannot bypass ownership or tenant boundary`, async () => {
      const orgs = scope === 'ORG' ? [h.root] : scope === 'ORG_SUBTREE' ? [h.root, h.child] : []
      replaceGrant(actor.state, 'update', scope, orgs)
      replaceGrant(actor.state, 'delete', scope, orgs)
      for (const [key, ticket] of Object.entries(h.tickets)) {
        if (keys.includes(key)) {
          const row = await accepted(h.url, actor.token, 'PATCH', path(ticket), { version: ticket.version, description: `Updated through ${scope}` })
          assert.equal(row.version, ticket.version + 1)
          assert.equal(row.tenantId, ticket.tenantId)
          assert.equal(row.organizationId, ticket.organizationId)
          assert.equal(row.ownerUserId, ticket.ownerUserId)
          h.tickets[key] = row
        } else {
          await denied(h.url, actor.token, 'PATCH', path(ticket), { version: ticket.version, title: 'Hidden resource mutation' }, [404])
          await denied(h.url, actor.token, 'DELETE', path(ticket), { version: ticket.version }, [404])
        }
      }
      const disposable = await createTicket(h.url, actor, `Soft delete ${scope}`)
      await accepted(h.url, actor.token, 'DELETE', path(disposable), { version: disposable.version })
      await denied(h.url, actor.token, 'GET', path(disposable), undefined, [404])
      await denied(h.url, actor.token, 'PATCH', path(disposable), { version: disposable.version, title: 'Restore via update' }, [404])
      await denied(h.url, actor.token, 'DELETE', path(disposable), { version: disposable.version }, [404])
      assert.ok(!(await records(h.url, actor)).some((row) => row.id === disposable.id))
      const csv = await call(h.url, actor.token, 'GET', `${API}/export.csv`)
      assert.equal(csv.status, 200)
      assert.ok(!csv.text.includes(disposable.title), 'deleted record remained exportable')
      const retained = await h.query('SELECT deleted_at FROM work_orders WHERE id=$1', [disposable.id])
      assert.equal(retained.rowCount, 1, 'delete physically removed the business record')
      assert.ok(retained.rows[0].deleted_at, 'delete failed to store deletion marker')
    })
  }

  await t.test('one concurrent version writer succeeds and stale writers cannot overwrite it', async () => {
    replaceGrant(actor.state, 'update', 'TENANT')
    replaceGrant(actor.state, 'delete', 'TENANT')
    const row = await createTicket(h.url, actor, 'Concurrent update')
    const responses = await Promise.all([
      call(h.url, actor.token, 'PATCH', path(row), { version: row.version, title: 'Winning title A' }),
      call(h.url, actor.token, 'PATCH', path(row), { version: row.version, title: 'Winning title B' }),
    ])
    assert.deepEqual(responses.map((response) => response.status).sort(), [200, 409])
    const winner = responses.find((response) => response.status === 200).value
    const stored = await accepted(h.url, actor.token, 'GET', path(row))
    assert.equal(stored.version, row.version + 1)
    assert.equal(stored.title, winner.title)
    await denied(h.url, actor.token, 'PATCH', path(row), { version: row.version, title: 'Stale overwrite' }, [409])
    await denied(h.url, actor.token, 'DELETE', path(row), { version: row.version }, [409])
    await accepted(h.url, actor.token, 'DELETE', path(row), { version: stored.version })
    const persisted = await h.query('SELECT version,deleted_at FROM work_orders WHERE id=$1', [row.id])
    assert.ok(persisted.rows[0].deleted_at)
    assert.equal(Number(persisted.rows[0].version), stored.version + 1)
  })

  await t.test('an authorization context that expires while waiting on a real row lock cannot commit a late write', async () => {
    const row = await createTicket(h.url, actor, 'Expiry while waiting for a PostgreSQL row lock')
    const blocker = await h.pool.connect()
    let pending
    try {
      await blocker.query('BEGIN')
      await blocker.query('SELECT id FROM work_orders WHERE id=$1 FOR UPDATE', [row.id])
      const expiry = Date.now() + 850
      actor.state.contextPatch = { expiresAt: new Date(expiry).toISOString() }
      pending = call(h.url, actor.token, 'PATCH', path(row), { version: row.version, title: 'Must not commit after expiry' })
      const deadline = Date.now() + 4000
      let waiting = false
      while (!waiting && Date.now() < deadline) {
        const state = await h.query("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT * FROM work_orders%') AS waiting")
        waiting = state.rows[0].waiting
        if (!waiting) await new Promise((resolve) => setTimeout(resolve, 20))
      }
      assert.ok(waiting, 'fixture did not prove the API was waiting on the protected PostgreSQL row lock')
      await new Promise((resolve) => setTimeout(resolve, Math.max(0, expiry - Date.now() + 20)))
      await blocker.query('ROLLBACK')
      const failed = await pending
      assert.equal(failed.status, 401)
      assert.equal(failed.value.code, 'INVALID_CONTEXT')
    } finally {
      actor.state.contextPatch = {}
      await blocker.query('ROLLBACK').catch(() => {})
      blocker.release()
      if (pending) await pending.catch(() => {})
    }
    const unchanged = await accepted(h.url, actor.token, 'GET', path(row))
    assert.equal(unchanged.version, row.version)
    assert.equal(unchanged.title, row.title)
  })

  for (const [method, column] of [['PATCH', 'title'], ['DELETE', 'deleted_at']]) {
    await t.test(`context expiry during final ${method} SQL rolls back version/title/deleted_at before commit`, async () => {
      const row = await createTicket(h.url, actor, `Rollback after slow final ${method} SQL`)
      assert.match(row.id, /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i)
      const fixtureName = `work_order_fixture_slow_${method.toLowerCase()}`
      let installed = false
      let pending
      try {
        // The API connection uses the same 5-second PostgreSQL statement budget
        // as createPool. Only this owned row is delayed, for 1.2 seconds.
        await h.query(`CREATE FUNCTION ${fixtureName}() RETURNS trigger LANGUAGE plpgsql AS $$
          BEGIN PERFORM pg_sleep(1.2); RETURN NEW; END $$`)
        await h.query(`CREATE TRIGGER ${fixtureName} BEFORE UPDATE OF ${column} ON work_orders
          FOR EACH ROW WHEN (OLD.id='${row.id}'::uuid) EXECUTE FUNCTION ${fixtureName}()`)
        installed = true
        actor.state.contextPatch = { expiresAt: new Date(Date.now() + 800).toISOString() }
        const body = { version: row.version, ...(method === 'PATCH' ? { title: 'Must roll back after SQL expiry' } : {}) }
        pending = call(h.url, actor.token, method, path(row), body)
        const deadline = Date.now() + 3000
        let executingSlowSql = false
        while (!executingSlowSql && Date.now() < deadline) {
          const state = await h.query("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='PgSleep' AND query LIKE 'UPDATE work_orders SET %') AS sleeping")
          executingSlowSql = state.rows[0].sleeping
          if (!executingSlowSql) await new Promise((resolve) => setTimeout(resolve, 20))
        }
        assert.ok(executingSlowSql, 'fixture did not prove the final mutation SQL executed its PostgreSQL sleep trigger')
        const failed = await pending
        assert.equal(failed.status, 401)
        assert.equal(failed.value.code, 'INVALID_CONTEXT')
      } finally {
        actor.state.contextPatch = {}
        if (pending) await pending.catch(() => {})
        if (installed) await h.query(`DROP TRIGGER ${fixtureName} ON work_orders`)
        await h.query(`DROP FUNCTION IF EXISTS ${fixtureName}()`)
      }
      const stored = await h.query('SELECT version,title,deleted_at,updated_at FROM work_orders WHERE id=$1', [row.id])
      assert.equal(stored.rowCount, 1)
      assert.equal(stored.rows[0].version, row.version)
      assert.equal(stored.rows[0].title, row.title)
      assert.equal(stored.rows[0].deleted_at, null)
      assert.equal(stored.rows[0].updated_at.toISOString(), row.updatedAt)
      const visible = await accepted(h.url, actor.token, 'GET', path(row))
      assert.equal(visible.version, row.version)
      assert.equal(visible.title, row.title)
    })
  }

  await t.test('status flow allows resolution and reopening and rejects invalid transitions atomically', async () => {
    let row = await createTicket(h.url, actor, 'State flow')
    await denied(h.url, actor.token, 'PATCH', path(row), { version: row.version, status: 'RESOLVED' }, [400, 409])
    for (const status of ['IN_PROGRESS', 'RESOLVED', 'IN_PROGRESS', 'RESOLVED']) {
      row = await accepted(h.url, actor.token, 'PATCH', path(row), { version: row.version, status })
      assert.equal(row.status, status)
    }
    const before = row.version
    await denied(h.url, actor.token, 'PATCH', path(row), { version: row.version, status: 'OPEN', title: 'Invalid state rollback' }, [400, 409])
    const current = await accepted(h.url, actor.token, 'GET', path(row))
    assert.equal(current.status, 'RESOLVED')
    assert.equal(current.title, 'State flow')
    assert.equal(current.version, before)
  })

  await t.test('SELF uses owner rather than assignee and permits own records across same-tenant organizations', async () => {
    replaceGrant(actor.state, 'read', 'SELF')
    replaceGrant(actor.state, 'update', 'SELF')
    replaceGrant(actor.state, 'delete', 'SELF')
    const assigned = await createTicket(h.url, h.actors.other, 'Assigned to actor but owned by another user', { assigneeUserId: actor.actor.userId })
    assert.equal(assigned.assigneeUserId, actor.actor.userId)
    await denied(h.url, actor.token, 'GET', path(assigned), undefined, [404])
    await denied(h.url, actor.token, 'PATCH', path(assigned), { version: assigned.version, title: 'Assignee bypass' }, [404])
    await denied(h.url, actor.token, 'DELETE', path(assigned), { version: assigned.version }, [404])
    const ownChild = await createTicket(h.url, h.actors.ownChild, 'Own child under SELF')
    const updated = await accepted(h.url, actor.token, 'PATCH', path(ownChild), { version: ownChild.version, priority: 'HIGH' })
    assert.equal(updated.organizationId, h.child)
    await accepted(h.url, actor.token, 'DELETE', path(updated), { version: updated.version })
    await denied(h.url, actor.token, 'GET', path(h.tickets.ownRemote), undefined, [404])
  })
})

test('real PG + HTTP platform fixture: strict input, Unicode limits and valid direct-member assignees', options, async (t) => {
  const h = await matrix(t, databaseUrl)
  const actor = h.actors.root
  const path = (ticket) => `${API}/${ticket.id}`
  await t.test('creation rejects client ownership, status, identity, timestamp and version overrides', async () => {
    const before = (await records(h.url, actor)).length
    const forbidden = {
      id: randomUUID(), number: 'FORGED-1', tenantId: h.tenantB, organizationId: h.child,
      ownerUserId: h.actors.other.actor.userId, status: 'RESOLVED', version: 123,
      createdAt: '2020-01-01T00:00:00Z', updatedAt: '2020-01-01T00:00:00Z', deletedAt: null,
      tenant_id: h.tenantB, organization_id: h.child, owner_user_id: h.actors.other.actor.userId,
      sourceKind: 'SYSTEM', arbitrary: true,
    }
    for (const [field, value] of Object.entries(forbidden)) {
      await denied(h.url, actor.token, 'POST', API, { title: 'Unauthorized field', [field]: value }, [400])
    }
    assert.equal((await records(h.url, actor)).length, before, 'rejected creation left partial records')
    const created = await createTicket(h.url, actor, 'Server-owned identity')
    assert.equal(created.tenantId, h.tenantA)
    assert.equal(created.organizationId, h.root)
    assert.equal(created.ownerUserId, actor.actor.userId)
    assert.equal(created.organizationName, 'Fixture Organization')
    assert.equal(created.ownerDisplayName, 'Fixture User')
    assert.equal(created.assigneeUserId, null)
    assert.equal(created.description, '')
    assert.ok(Number.isFinite(Date.parse(created.createdAt)) && Number.isFinite(Date.parse(created.updatedAt)))
  })

  await t.test('PATCH and DELETE require positive integer versions and reject ownership movement', async () => {
    const row = await createTicket(h.url, actor, 'Immutable owner test')
    for (const field of ['tenantId', 'organizationId', 'ownerUserId', 'id', 'number', 'createdAt', 'updatedAt', 'deletedAt', 'arbitrary']) {
      await denied(h.url, actor.token, 'PATCH', path(row), { version: row.version, [field]: randomUUID() }, [400])
    }
    for (const version of [undefined, null, 0, -1, 1.5, '1', true, Number.MAX_SAFE_INTEGER + 1]) {
      const body = version === undefined ? { title: 'Missing version' } : { version, title: 'Bad version' }
      await denied(h.url, actor.token, 'PATCH', path(row), body, [400])
      await denied(h.url, actor.token, 'DELETE', path(row), version === undefined ? {} : { version }, [400])
    }
    await denied(h.url, actor.token, 'DELETE', path(row), { version: row.version, tenantId: h.tenantB }, [400])
    const unchanged = await accepted(h.url, actor.token, 'GET', path(row))
    assert.equal(unchanged.version, row.version)
    assert.equal(unchanged.title, row.title)
    assert.equal(unchanged.organizationId, row.organizationId)
  })

  await t.test('field caps count Unicode code points and pure text remains intact', async () => {
    const title = '😀'.repeat(200)
    const description = '中'.repeat(8192)
    const valid = await createTicket(h.url, actor, title, { description, priority: 'LOW' })
    assert.equal(valid.title, title)
    assert.equal(valid.description, description)
    assert.equal(valid.priority, 'LOW')
    const trimmed = await createTicket(h.url, actor, '  标题  ', { description: '<script>alert(1)</script>' })
    assert.equal(trimmed.title, '标题')
    assert.equal(trimmed.description, '<script>alert(1)</script>')
    for (const body of [
      {}, { title: '' }, { title: '   ' }, { title: null }, { title: 12 },
      { title: '😀'.repeat(201) }, { title: 'Too much', description: '😀'.repeat(8193) },
      { title: 'Wrong description', description: null }, { title: 'Wrong description', description: {} },
      { title: 'Bad priority', priority: 'URGENT' }, { title: 'Bad priority', priority: null },
    ]) await denied(h.url, actor.token, 'POST', API, body, [400])
    await denied(h.url, actor.token, 'PATCH', path(valid), { version: valid.version, description: '😀'.repeat(8193) }, [400])
    const stillValid = await accepted(h.url, actor.token, 'GET', path(valid))
    assert.equal(stillValid.version, valid.version)
    assert.equal(stillValid.description, description)
  })

  await t.test('malformed and oversized HTTP bodies fail before inserting or changing records', async () => {
    const before = (await records(h.url, actor)).length
    for (const raw of ['{ invalid JSON', '[]', 'null']) {
      await denied(h.url, actor.token, 'POST', API, raw, [400])
    }
    await denied(h.url, actor.token, 'POST', API, { title: 'Oversized request', description: 'x'.repeat(70 * 1024) }, [400, 413])
    const result = await call(h.url, actor.token, 'POST', API, { title: 'Wrong media type' }, { headers: { 'content-type': 'text/plain' } })
    assert.ok([400, 415].includes(result.status))
    assert.equal((await records(h.url, actor)).length, before)
  })

  await t.test('assignee must be an effective direct member of the record organization', async () => {
    const validAssignee = h.actors.other.actor.userId
    const row = await createTicket(h.url, actor, 'Valid assignee', { assigneeUserId: validAssignee })
    assert.equal(row.assigneeUserId, validAssignee)
    assert.equal(row.assigneeDisplayName, `Member ${validAssignee.slice(0, 8)}`)
    for (const assigneeUserId of [randomUUID(), h.actors.child.actor.userId, h.actors.remote.actor.userId, 'not-a-uuid']) {
      await denied(h.url, actor.token, 'POST', API, { title: 'Invalid assignee', assigneeUserId }, [400, 403, 404])
      await denied(h.url, actor.token, 'PATCH', path(row), { version: row.version, assigneeUserId }, [400, 403, 404])
    }
    h.platform.members.delete(`${h.tenantA}:${h.root}:${validAssignee}`)
    const previouslyUnassigned = await createTicket(h.url, actor, 'New assignment after member removal')
    await denied(h.url, actor.token, 'POST', API, { title: 'Removed member assignment', assigneeUserId: validAssignee }, [400, 403, 404])
    await denied(h.url, actor.token, 'PATCH', path(previouslyUnassigned), { version: previouslyUnassigned.version, assigneeUserId: validAssignee }, [400, 403, 404])
    const unchanged = await accepted(h.url, actor.token, 'GET', path(row))
    assert.equal(unchanged.version, row.version)
    const unassigned = await accepted(h.url, actor.token, 'PATCH', path(row), { version: row.version, assigneeUserId: null })
    assert.equal(unassigned.assigneeUserId, null)
    assert.equal(unassigned.assigneeDisplayName, null)
    for (const patch of [{ id: h.actors.child.actor.userId }, { organizationId: h.child }]) {
      actor.state.memberPatch = patch
      await denied(h.url, actor.token, 'POST', API, { title: 'Misbound membership result', assigneeUserId: actor.actor.userId }, [400, 403, 503])
    }
    actor.state.memberPatch = {}
  })

  await t.test('assignee directory has independent action authorization and limits selection to allowed organizations', async () => {
    const directory = '/work-orders-api/assignees'
    replaceGrant(actor.state, 'create', 'SELF')
    const current = await accepted(h.url, actor.token, 'GET', `${directory}?action=create&page=1&size=50`)
    assert.ok(current.records.length > 0)
    assert.ok(current.records.every((row) => row.organizationId === h.root && typeof row.displayName === 'string'))
    assert.ok(current.records.every((row) => /^[0-9a-f-]{36}$/i.test(row.userId)))
    await denied(h.url, actor.token, 'GET', `${directory}?action=create&organizationId=${h.child}`, undefined, [403])
    replaceGrant(actor.state, 'create', 'ORG_SUBTREE', [h.root, h.child])
    const descendant = await accepted(h.url, actor.token, 'GET', `${directory}?action=create&organizationId=${h.child}&size=50`)
    assert.deepEqual(descendant.records.map((row) => row.userId).sort(), [actor.actor.userId, h.actors.child.actor.userId].sort())
    await denied(h.url, actor.token, 'GET', `${directory}?action=create&organizationId=${h.peer}`, undefined, [403])
    replaceGrant(actor.state, 'create', 'SELF')
    await denied(h.url, actor.token, 'POST', API, { title: 'Directory did not permit moving creation', assigneeUserId: h.actors.child.actor.userId }, [404])
    replaceGrant(actor.state, 'create', null)
    await denied(h.url, actor.token, 'GET', `${directory}?action=create`, undefined, [403])
    replaceGrant(actor.state, 'create', 'TENANT')
    for (const query of ['', '?action=read', '?action=create&size=51', '?action=create&page=0', `?action=create&q=${'x'.repeat(101)}`, `?action=create&tenantId=${h.tenantB}`]) {
      await denied(h.url, actor.token, 'GET', `${directory}${query}`, undefined, [400])
    }
    actor.state.mode.assignee = 500
    await denied(h.url, actor.token, 'GET', `${directory}?action=create`, undefined, [503])
    delete actor.state.mode.assignee
    await accepted(h.url, actor.token, 'GET', `${directory}?action=create`)
  })

  await t.test('production migrations are idempotent and persistent identity cannot be moved by SQL', async () => {
    const { migrate } = await import('../db.mjs')
    await migrate(h.pool)
    const migrationCount = await h.query('SELECT count(*)::integer AS count FROM work_order_migrations')
    assert.equal(migrationCount.rows[0].count, 1)
    const row = h.tickets.ownRoot
    for (const [column, value] of [['tenant_id', h.tenantB], ['organization_id', h.child], ['owner_user_id', h.actors.other.actor.userId]]) {
      await assert.rejects(h.query(`UPDATE work_orders SET ${column}=$2 WHERE id=$1`, [row.id, value]), /23514/)
    }
    const saved = await accepted(h.url, actor.token, 'GET', `${API}/${row.id}`)
    assert.equal(saved.tenantId, row.tenantId)
    assert.equal(saved.organizationId, row.organizationId)
    assert.equal(saved.ownerUserId, row.ownerUserId)
    assert.equal(saved.version, row.version)
  })
})

function parseCsv(text) {
  const rows = []
  let row = []
  let field = ''
  let quoted = false
  const source = text.replace(/^\uFEFF/, '')
  for (let i = 0; i < source.length; i += 1) {
    const character = source[i]
    if (character === '"') {
      if (quoted && source[i + 1] === '"') { field += '"'; i += 1 }
      else quoted = !quoted
    } else if (character === ',' && !quoted) { row.push(field); field = '' }
    else if ((character === '\n' || character === '\r') && !quoted) {
      if (character === '\r' && source[i + 1] === '\n') i += 1
      row.push(field); rows.push(row); row = []; field = ''
    } else field += character
  }
  if (field || row.length) { row.push(field); rows.push(row) }
  assert.equal(quoted, false, 'CSV ended inside a quoted field')
  return rows
}

test('real PG + HTTP platform fixture: independent export authority and safe persistent CSV', options, async (t) => {
  const h = await matrix(t, databaseUrl)
  const actor = h.actors.root
  await t.test('read authority cannot substitute for export permission or export data grant', async () => {
    replaceGrant(actor.state, 'read', 'TENANT')
    replaceGrant(actor.state, 'export', 'TENANT')
    const original = actor.state.permissions
    actor.state.permissions = original.filter((permission) => permission !== 'ticket:export')
    await denied(h.url, actor.token, 'GET', `${API}/export.csv`, undefined, [403])
    await accepted(h.url, actor.token, 'GET', API)
    actor.state.permissions = original
    replaceGrant(actor.state, 'export', null)
    await denied(h.url, actor.token, 'GET', `${API}/export.csv`, undefined, [403])
    replaceGrant(actor.state, 'export', 'ORG', [h.root])
    actor.state.permissions = original.filter((permission) => permission !== 'ticket:read')
    replaceGrant(actor.state, 'read', null)
    await denied(h.url, actor.token, 'GET', API, undefined, [403])
    const csv = await call(h.url, actor.token, 'GET', `${API}/export.csv`)
    assert.equal(csv.status, 200, 'independent export incorrectly required read permission or read grant')
    assert.ok(csv.text.includes(h.tickets.ownRoot.title) && csv.text.includes(h.tickets.otherRoot.title))
    assert.ok(!csv.text.includes(h.tickets.ownChild.title) && !csv.text.includes(h.tickets.remote.title))
    actor.state.permissions = original
    replaceGrant(actor.state, 'read', 'TENANT')
  })

  await t.test('CSV safely quotes Unicode, commas, quotes, newlines and neutralizes formula prefixes', async () => {
    replaceGrant(actor.state, 'export', 'TENANT')
    const formulas = ['=1+1', '+SUM(1,2)', '-2+3', '@SUM(1,2)', '\t=1+1', '\r=1+1']
    const created = []
    for (const description of formulas) created.push(await createTicket(h.url, actor, `Formula fixture ${created.length}`, { description }))
    const titleFormula = await createTicket(h.url, actor, '=SUM(2,3)', { description: 'Formula in title' })
    const complex = '中文,"quoted"\r\nsecond line 😀'
    const complexRow = await createTicket(h.url, actor, '中文 CSV 😀', { description: complex })
    const csv = await call(h.url, actor.token, 'GET', `${API}/export.csv`)
    assert.equal(csv.status, 200)
    assert.match(csv.headers.get('content-disposition') ?? '', /attachment/i)
    const rows = parseCsv(csv.text)
    const header = rows.shift()
    assert.ok(header.length >= 5)
    assert.ok(rows.every((row) => row.length === header.length), 'CSV row field count differs from its header')
    const descriptionColumn = header.findIndex((name) => /^(description|描述)$/i.test(name))
    const titleColumn = header.findIndex((name) => /^(title|标题)$/i.test(name))
    assert.ok(descriptionColumn >= 0 && titleColumn >= 0, 'CSV needs identifiable title and description columns')
    for (let i = 0; i < created.length; i += 1) {
      const exported = rows.find((row) => row[titleColumn] === created[i].title)
      assert.ok(exported)
      const text = exported[descriptionColumn]
      assert.ok(!/^[=+\-@\t\r]/.test(text), 'CSV exported an active spreadsheet formula prefix')
      assert.ok(text.includes(formulas[i]), 'CSV mitigation destroyed the original text')
    }
    assert.equal(rows.find((row) => row[titleColumn] === complexRow.title)[descriptionColumn], complex)
    const exportedTitle = rows.find((row) => row[descriptionColumn] === titleFormula.description)[titleColumn]
    assert.ok(!/^[=+\-@\t\r]/.test(exportedTitle))
    assert.ok(exportedTitle.includes(titleFormula.title))
    assert.ok(!csv.text.includes(h.tickets.remote.title), 'CSV crossed tenant boundary')
    for (const row of created) {
      const saved = await accepted(h.url, actor.token, 'GET', `${API}/${row.id}`)
      assert.equal(saved.description, row.description, 'CSV mitigation altered the persistent business data')
    }
  })

  await t.test('CSV capacity limits reject the complete export without truncating persistent data', async () => {
    replaceGrant(actor.state, 'export', 'TENANT')
    const insert = `INSERT INTO work_orders
      (id,number,tenant_id,organization_id,organization_name,owner_user_id,owner_display_name,title,description)
      SELECT gen_random_uuid(),'capacity-fixture-'||g,$1,$2,'Fixture Organization',$3,'Fixture User',
      'Capacity fixture '||g,$4 FROM generate_series(1,$5::integer) AS g`
    await h.query(insert, [h.tenantA, h.root, actor.actor.userId, '', 1001])
    const limitedRows = await denied(h.url, actor.token, 'GET', `${API}/export.csv`, undefined, [400])
    assert.equal(limitedRows.value.code, 'EXPORT_LIMIT')
    const retained = await h.query("SELECT count(*)::integer AS count FROM work_orders WHERE number LIKE 'capacity-fixture-%'")
    assert.equal(retained.rows[0].count, 1001)
    await h.query("DELETE FROM work_orders WHERE number LIKE 'capacity-fixture-%'")
    await h.query(insert, [h.tenantA, h.root, actor.actor.userId, 'x'.repeat(8192), 600])
    // This case transfers a real 4.9 MB PostgreSQL result over the remote test
    // connection before the API can reject the serialized CSV. Its client wait
    // must remain within the platform context TTL while allowing that transfer.
    const limitedBytes = await call(h.url, actor.token, 'GET', `${API}/export.csv`, undefined, { timeoutMs: 30_000 })
    assert.equal(limitedBytes.status, 413)
    assert.equal(limitedBytes.value.code, 'EXPORT_LIMIT')
    const stillThere = await h.query("SELECT count(*)::integer AS count FROM work_orders WHERE number LIKE 'capacity-fixture-%'")
    assert.equal(stillThere.rows[0].count, 600)
  })
})

test('real PG + HTTP platform fixture: protocol binding and upstream transport failures fail closed', options, async (t) => {
  const h = await matrix(t, databaseUrl)
  const actor = h.actors.root
  await t.test('missing credentials and inactive/wrong-audience/wrong-type/expired introspection reject', async () => {
    await denied(h.url, '', 'GET', API, undefined, [401])
    await denied(h.url, 'not-in-fixture', 'GET', API, undefined, [401])
    for (const patch of [
      { active: false }, { active: 'true' }, { aud: 'another-app' }, { aud: ['work-orders'] },
      { sub: '' }, { tenant_id: '' }, { organization_id: '' }, { sub: 'invalid-uuid' },
      { token_type: 'refresh' }, { exp: Math.floor(Date.now() / 1000) - 1 }, { exp: 'not-a-time' },
    ]) {
      actor.state.introPatch = patch
      await denied(h.url, actor.token, 'GET', API, undefined, [401, 503])
    }
    actor.state.introPatch = {}
    await accepted(h.url, actor.token, 'GET', API)
  })

  await t.test('sandbox iframe origin is explicit and CORS does not substitute for token authentication', async () => {
    const before = actor.state.calls.introspect
    const preflight = await call(h.url, '', 'OPTIONS', API, undefined, { headers: {
      origin: 'null', 'access-control-request-method': 'PATCH', 'access-control-request-headers': 'authorization,content-type',
    } })
    assert.equal(preflight.status, 204)
    assert.equal(preflight.headers.get('access-control-allow-origin'), 'null')
    assert.equal(preflight.headers.get('access-control-allow-credentials'), null)
    assert.equal(actor.state.calls.introspect, before, 'preflight attempted bearer-token authorization')
    const authenticated = await call(h.url, actor.token, 'GET', API, undefined, { headers: { origin: 'null' } })
    assert.equal(authenticated.status, 200)
    assert.equal(authenticated.headers.get('access-control-allow-origin'), 'null')
    const missing = await call(h.url, '', 'GET', API, undefined, { headers: { origin: 'null' } })
    assert.equal(missing.status, 401)
    const unexpectedOrigin = await call(h.url, actor.token, 'GET', API, undefined, { headers: { origin: 'https://unapproved.example.test' } })
    assert.equal(unexpectedOrigin.status, 403)
    assert.equal(unexpectedOrigin.headers.get('access-control-allow-origin'), null)
  })

  await t.test('context contract, audience, identity, tenant, org and TTL bind to introspected token', async () => {
    for (const patch of [
      { contractVersion: '2.0' }, { contextVersion: '' }, { appCode: 'another-app' },
      { user: { id: h.actors.other.actor.userId } }, { tenant: { id: h.tenantB } }, { organization: { id: h.child } },
      { expiresAt: new Date(Date.now() - 1).toISOString() }, { expiresAt: 'not-a-date' },
      { expiresAt: new Date(Date.now() + 60 * 60 * 1000).toISOString() },
    ]) {
      actor.state.contextPatch = patch
      await denied(h.url, actor.token, 'GET', API, undefined, [401, 503])
    }
    actor.state.contextPatch = {}
    await accepted(h.url, actor.token, 'GET', API)
  })

  await t.test('malformed or misbound grants do not become broad SQL scopes', async () => {
    const original = actor.state.grants
    for (const grant of [
      { ...grantFor(actor.actor, 'read'), tenantId: h.tenantB },
      { ...grantFor(actor.actor, 'read'), subjectUserId: h.actors.other.actor.userId },
      { ...grantFor(actor.actor, 'read'), scope: 'ALL' },
      { ...grantFor(actor.actor, 'read', 'ORG'), organizationIds: [h.peer] },
      { ...grantFor(actor.actor, 'read', 'ORG_SUBTREE'), organizationIds: 'not-an-array' },
      { ...grantFor(actor.actor, 'read', 'ORG_SUBTREE'), organizationIds: [] },
      { ...grantFor(actor.actor, 'read', 'ORG_SUBTREE'), organizationIds: [h.root, 'invalid-uuid'] },
    ]) {
      actor.state.grants = [grant]
      await denied(h.url, actor.token, 'GET', API, undefined, [401, 403, 503])
    }
    actor.state.grants = original
    await accepted(h.url, actor.token, 'GET', API)
  })

  await t.test('operation role with no direct data grants is denied even with inherited role metadata', async () => {
    const original = actor.state.grants
    actor.state.grants = []
    actor.state.contextPatch = { roles: [{ id: randomUUID(), name: 'Inherited role', code: 'ancestor-role' }] }
    for (const action of ACTIONS) {
      const method = action === 'create' ? 'POST' : action === 'update' ? 'PATCH' : action === 'delete' ? 'DELETE' : 'GET'
      const path = action === 'export' ? `${API}/export.csv` : ['update', 'delete'].includes(action) ? `${API}/${h.tickets.ownRoot.id}` : API
      const body = action === 'create' ? { title: 'Denied inherited role creation' } : ['update', 'delete'].includes(action) ? { version: 1, ...(action === 'update' ? { title: 'Denied inherited role update' } : {}) } : undefined
      await denied(h.url, actor.token, method, path, body, [403])
    }
    actor.state.grants = original
    actor.state.contextPatch = {}
  })

  await t.test('redirects, timeouts, broken sockets, invalid JSON, oversized platform bodies and HTTP errors are closed', async () => {
    const initial = h.tickets.ownRoot
    const beforeCount = (await records(h.url, actor)).length
    for (const stage of ['introspect', 'context']) {
      for (const mode of ['redirect', 'timeout', 'network', 'invalid-json', 'oversized', 401, 403, 500]) {
        actor.state.mode[stage] = mode
        const started = Date.now()
        await denied(h.url, actor.token, 'POST', API, { title: `Should not exist ${stage} ${mode}` }, [401, 403, 503])
        assert.ok(Date.now() - started < 3000, 'upstream failure exceeded configured fail-closed time budget')
        delete actor.state.mode[stage]
      }
    }
    assert.equal(h.platform.trapCount, 0, 'business service followed a platform authentication redirect')
    assert.equal((await records(h.url, actor)).length, beforeCount)
    assert.equal((await accepted(h.url, actor.token, 'GET', `${API}/${initial.id}`)).version, initial.version)
  })

})

function reminderGrant(actor, scope = 'TENANT', organizationIds = []) {
  actor.state.permissions = [...new Set([...actor.state.permissions, 'notification:publish'])]
  actor.state.grants = actor.state.grants.filter((grant) => grant.resource !== 'notification')
  actor.state.grants.push({ ...grantFor(actor.actor, 'publish', scope, organizationIds), resource: 'notification' })
}

test('real PG + HTTP platform fixture: manual reminders authorize separately and never save the ticket twice', options, async (t) => {
  const h = await matrix(t, databaseUrl)
  const actor = h.actors.root
  const initial = h.tickets.ownRoot
  const path = `${API}/${initial.id}/remind`
  const body = { idempotencyKey: 'ticket-reminder-event-1' }

  await t.test('reading a ticket does not grant notification publication or notification data access', async () => {
    await denied(h.url, actor.token, 'POST', path, body, [403])
    reminderGrant(actor)
    actor.state.permissions = actor.state.permissions.filter((permission) => permission !== 'notification:publish')
    await denied(h.url, actor.token, 'POST', path, body, [403])
    reminderGrant(actor)
    actor.state.grants = actor.state.grants.filter((grant) => grant.resource !== 'notification')
    await denied(h.url, actor.token, 'POST', path, body, [403])
    assert.equal(h.platform.notifications.length, 0)
  })

  await t.test('a manual event forwards stable scoped payload and retry reuses its original publication', async () => {
    reminderGrant(actor, 'SELF')
    const first = await accepted(h.url, actor.token, 'POST', path, body)
    assert.equal(first.duplicate, false)
    assert.equal(first.recipientCount, 1)
    assert.equal(first.deliveryCount, 1)
    const forwarded = h.platform.notifications.at(-1)
    assert.deepEqual(forwarded, {
      idempotencyKey: body.idempotencyKey,
      target: { kind: 'USER', userId: actor.actor.userId, organizationId: h.root },
      type: 'INFO', title: `工单提醒 ${initial.number}`,
      content: `请查看工单 ${initial.number} 的详情并处理。`, link: `/micro/work-orders/tickets/${initial.id}`,
    })
    const second = await accepted(h.url, actor.token, 'POST', path, body)
    assert.equal(second.duplicate, true)
    for (const key of ['publicationId', 'recipientCount', 'deliveryCount', 'createdAt']) assert.equal(second[key], first[key])
    const ticket = await accepted(h.url, actor.token, 'GET', `${API}/${initial.id}`)
    assert.equal(ticket.version, initial.version)
    assert.equal(ticket.updatedAt, initial.updatedAt)
  })

  await t.test('assignee is the target and changing target under the same event key conflicts', async () => {
    reminderGrant(actor, 'ORG', [h.root])
    const updated = await accepted(h.url, actor.token, 'PATCH', `${API}/${initial.id}`, { version: initial.version, assigneeUserId: h.actors.other.actor.userId })
    await denied(h.url, actor.token, 'POST', path, body, [409])
    const fresh = await accepted(h.url, actor.token, 'POST', path, { idempotencyKey: 'ticket-reminder-event-2' })
    assert.equal(fresh.duplicate, false)
    assert.deepEqual(h.platform.notifications.at(-1).target, { kind: 'USER', userId: h.actors.other.actor.userId, organizationId: h.root })
    const saved = await accepted(h.url, actor.token, 'GET', `${API}/${initial.id}`)
    assert.equal(saved.version, updated.version)
    assert.equal(saved.updatedAt, updated.updatedAt)
  })

  await t.test('reminder target cannot bypass notification SELF or ticket visibility', async () => {
    reminderGrant(actor, 'SELF')
    await denied(h.url, actor.token, 'POST', path, { idempotencyKey: 'self-cannot-remind-other' }, [403])
    reminderGrant(actor)
    await denied(h.url, actor.token, 'POST', `${API}/${h.tickets.remote.id}/remind`, { idempotencyKey: 'cross-tenant-reminder' }, [404])
    replaceGrant(actor.state, 'read', 'SELF')
    await denied(h.url, actor.token, 'POST', `${API}/${h.tickets.otherRoot.id}/remind`, { idempotencyKey: 'hidden-reminder' }, [404])
    replaceGrant(actor.state, 'read', 'TENANT')
    for (const invalid of [{}, { idempotencyKey: '' }, { idempotencyKey: 'bad key' }, { idempotencyKey: 'x'.repeat(129) },
      { idempotencyKey: 'forged', target: { kind: 'TENANT' } }, { idempotencyKey: 'forged', title: 'Caller chosen reminder' },
      { idempotencyKey: 'forged', userId: h.actors.remote.actor.userId }]) {
      await denied(h.url, actor.token, 'POST', path, invalid, [400])
    }
  })

  await t.test('failed reminder publication preserves the ticket and retries the same event key', async () => {
    reminderGrant(actor)
    const before = await accepted(h.url, actor.token, 'GET', `${API}/${initial.id}`)
    for (const failure of [400, 401, 403, 409, 500, 'network', 'timeout']) {
      actor.state.mode.notification = failure
      const expected = typeof failure === 'number' && failure < 500 ? [failure] : [503]
      await denied(h.url, actor.token, 'POST', path, { idempotencyKey: 'retry-after-publication-failure' }, expected)
      delete actor.state.mode.notification
    }
    const retried = await accepted(h.url, actor.token, 'POST', path, { idempotencyKey: 'retry-after-publication-failure' })
    assert.equal(retried.duplicate, false)
    const after = await accepted(h.url, actor.token, 'GET', `${API}/${initial.id}`)
    assert.equal(after.version, before.version)
    assert.equal(after.updatedAt, before.updatedAt)
    assert.equal(after.assigneeUserId, before.assigneeUserId)
  })
})
