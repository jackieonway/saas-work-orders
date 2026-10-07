import { loadConfig } from './config.mjs'
import { createPool, migrate } from './db.mjs'
import { createServer } from './server.mjs'

let pool
try {
  const config = loadConfig()
  pool = createPool(config)
  pool.on('error', () => console.error('work-orders-api database connection unavailable'))
  await migrate(pool)
  const server = createServer({ config, pool })
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(config.port, config.host, resolve) })
  console.info(JSON.stringify({ event: 'work-orders-api-ready', host: config.host, port: server.address().port }))
  let stopping = false
  async function stop() {
    if (stopping) return
    stopping = true
    const timer = setTimeout(() => { server.closeAllConnections(); process.exit(1) }, 10000).unref()
    await new Promise((resolve) => server.close(resolve))
    await pool.end()
    clearTimeout(timer)
  }
  process.once('SIGTERM', stop)
  process.once('SIGINT', stop)
} catch {
  console.error('work-orders-api startup failed; check business database, migrations and required configuration')
  await pool?.end().catch(() => {})
  process.exitCode = 1
}
