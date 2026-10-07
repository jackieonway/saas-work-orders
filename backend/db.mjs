import { readFile, readdir } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import pg from 'pg'

export function createPool(config) {
  return new pg.Pool({ connectionString: config.databaseUrl, max: 10,
    connectionTimeoutMillis: 5000, idleTimeoutMillis: 30000,
    statement_timeout: 5000, idle_in_transaction_session_timeout: 15000 })
}

/** Independent, checksummed, transactionally applied business migrations. */
export async function migrate(pool) {
  const client = await pool.connect()
  try {
    await client.query('BEGIN')
    await client.query("SELECT pg_advisory_xact_lock(hashtextextended('work-orders-business-migrations',11))")
    await client.query('CREATE TABLE IF NOT EXISTS work_order_migrations(version TEXT PRIMARY KEY,checksum CHAR(64) NOT NULL,applied_at TIMESTAMPTZ NOT NULL DEFAULT now())')
    const directory = new URL('./migrations/', import.meta.url)
    const files = (await readdir(directory)).filter((file) => /^\d{4}_[a-z_]+\.sql$/.test(file)).sort()
    for (const file of files) {
      const sql = await readFile(new URL(file, directory), 'utf8')
      const checksum = createHash('sha256').update(sql).digest('hex')
      const previous = await client.query('SELECT checksum FROM work_order_migrations WHERE version=$1', [file])
      if (previous.rowCount) {
        if (previous.rows[0].checksum !== checksum) throw new Error('Applied business migration checksum differs; startup stopped')
        continue
      }
      await client.query(sql)
      await client.query('INSERT INTO work_order_migrations(version,checksum) VALUES($1,$2)', [file, checksum])
    }
    await client.query('COMMIT')
  } catch (error) {
    await client.query('ROLLBACK').catch(() => {})
    throw error
  } finally { client.release() }
}
