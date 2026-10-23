import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { build, preview } from 'vite'
import { createWorkOrdersConfig, readPublicConfig } from '../vite.config.mjs'

// Serve the same classic UMD entry as production; native Vite ESM cannot be
// loaded as a qiankun 2 entry. Rebuild source changes, then refresh the host.
const root = fileURLToPath(new URL('../', import.meta.url))
execFileSync(process.execPath, ['scripts/build.mjs'], { cwd: root, stdio: 'inherit', windowsHide: true })
const config = createWorkOrdersConfig(readPublicConfig())
const watcher = await build({ ...config, configFile: false, build: { ...config.build, watch: {} } })
const server = await preview({ ...config, configFile: false })
server.printUrls()
let closing = false
async function close() {
  if (closing) return
  closing = true
  await watcher.close()
  server.httpServer.closeAllConnections()
  await new Promise(resolve => server.httpServer.close(resolve))
}
process.once('SIGINT', close)
process.once('SIGTERM', close)
