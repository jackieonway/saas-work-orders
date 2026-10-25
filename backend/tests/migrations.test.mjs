import assert from 'node:assert/strict'
import test from 'node:test'
import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { migrate } from '../db.mjs'

test('Node compatibility runner appends UUID v7 migration to the unchanged three-file history', async () => {
  const history = new Map()
  for (const name of ['0001_work_orders.sql', '0002_create_idempotency.sql', '0003_reminder_intents.sql']) {
    const sql = await readFile(new URL('../migrations/' + name, import.meta.url), 'utf8')
    history.set(name, createHash('sha256').update(sql).digest('hex'))
  }
  const applied = []
  const client = {
    async query(sql, values = []) {
      if (sql.startsWith('SELECT checksum')) {
        return { rowCount: history.has(values[0]) ? 1 : 0, rows: history.has(values[0]) ? [{ checksum: history.get(values[0]) }] : [] }
      }
      if (sql.startsWith('INSERT INTO work_order_migrations')) {
        history.set(values[0], values[1]); applied.push(values[0])
      }
      return { rows: [], rowCount: 0 }
    },
    release() {},
  }
  const pool = { async connect() { return client } }
  await migrate(pool)
  assert.deepEqual(applied, ['0004_uuid_v7.sql'])
  await migrate(pool)
  assert.deepEqual(applied, ['0004_uuid_v7.sql'], 'rerun must preserve every applied checksum')
})
