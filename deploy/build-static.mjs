import { execFileSync } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'
import { pathToFileURL } from 'node:url'
import { renderBusinessStatic, validateStaticInput } from './render-static.mjs'

// Keep the public four-field input separate from derived validation fields.
// renderBusinessStatic validates its closed input itself; apiOrigin is derived
// internally and must never be fed back as an extra public configuration key.
export function createStaticBuildPlan(input) {
  const config = validateStaticInput(input)
  return {
    args: ['scripts/build.mjs', '--version', config.version,
      '--allowed-host-origin', config.hostOrigin, '--api-base', config.apiBase],
    nginx: renderBusinessStatic(input),
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  // Build-stage only. These four public values are the complete browser input.
  const plan = createStaticBuildPlan({ appCode: process.env.APP_CODE, version: process.env.APP_VERSION,
    hostOrigin: process.env.HOST_ORIGIN, apiBase: process.env.API_BASE })
  execFileSync(process.execPath, plan.args, { stdio: 'inherit' })
  mkdirSync('/out', { recursive: true })
  writeFileSync('/out/nginx.conf', plan.nginx, { mode: 0o644 })
}
