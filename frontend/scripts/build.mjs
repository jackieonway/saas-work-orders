import { existsSync, lstatSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import os from 'node:os'
import { fileURLToPath } from 'node:url'
import { build } from 'vite'
import { createWorkOrdersConfig, readPublicConfig } from '../vite.config.mjs'
import { checkRelease, releaseFiles, releaseHashFile } from './release-check.mjs'

const appRoot = fileURLToPath(new URL('../', import.meta.url))
const args = process.argv.slice(2), options = { origins: [] }
for (let index = 0; index < args.length; index += 2) {
  const name = args[index], value = args[index + 1]
  if (!value || !['--out-dir', '--version', '--allowed-host-origin', '--api-base'].includes(name)) throw new Error('未知构建参数或缺少值')
  if (name === '--allowed-host-origin') options.origins.push(value)
  else options[name.slice(2)] = value
}
const packageVersion = JSON.parse(readFileSync(path.join(appRoot, 'package.json'), 'utf8')).version
const publicConfig = readPublicConfig({ ...options, version: options.version || process.env.WORK_ORDERS_VERSION || packageVersion })

// Resolve existing ancestors before allowing any recursive operation. A symlink
// must not turn a permitted temporary path into an unrelated directory.
function canonicalDestination(input) {
  let existing = path.resolve(input), suffix = []
  while (!existsSync(existing)) {
    suffix.unshift(path.basename(existing))
    const parent = path.dirname(existing)
    if (parent === existing) throw new Error('输出目录无法解析')
    existing = parent
  }
  if (lstatSync(existing).isSymbolicLink()) throw new Error('输出目录不得使用符号链接')
  return path.join(realpathSync(existing), ...suffix)
}
const requested = path.resolve(options['out-dir'] || path.join(appRoot, 'dist'))
const destination = canonicalDestination(requested)
const defaultDestination = path.join(realpathSync(appRoot), 'dist')
const tempRelative = path.relative(realpathSync(os.tmpdir()), destination)
if (destination !== defaultDestination && (!tempRelative || path.isAbsolute(tempRelative) || tempRelative === '..' || tempRelative.startsWith('..' + path.sep))) {
  throw new Error('自定义输出须位于系统临时目录内；发布请打包生成的 dist')
}
if (existsSync(requested) && lstatSync(requested).isSymbolicLink()) throw new Error('输出目录不得使用符号链接')
if (existsSync(destination)) {
  let owned = false
  try { owned = JSON.parse(readFileSync(path.join(destination, 'build-info.json'), 'utf8')).application === 'work-orders' } catch {}
  if (!owned) throw new Error('输出目录不属于本应用构建，拒绝覆盖')
  rmSync(destination, { recursive: true, force: true })
}
mkdirSync(destination, { recursive: true })
const config = createWorkOrdersConfig(publicConfig)
await build({ ...config, configFile: false, build: { ...config.build, outDir: destination } })
const metadataPath = path.join(destination, 'build-info.json')
const metadata = JSON.parse(readFileSync(metadataPath, 'utf8'))
writeFileSync(metadataPath, JSON.stringify({ ...metadata, format: 'umd', releaseGateVersion: 1, packageVersion, publicConfig }, null, 2) + '\n')
writeFileSync(path.join(destination, releaseHashFile), JSON.stringify({ schemaVersion: 1, application: 'work-orders',
  version: publicConfig.version, files: releaseFiles(destination) }, null, 2) + '\n')
const report = await checkRelease(destination, { expectedVersion: publicConfig.version, expectedPublicConfig: publicConfig })
console.log(`Release gate passed: ${report.runtime}, SDK ${report.sdkVersion}, ${report.files} files`)
console.log(`工单应用 ${publicConfig.version} 已构建：${destination}`)
