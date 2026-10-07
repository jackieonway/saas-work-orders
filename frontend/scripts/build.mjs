import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import path from 'node:path'
import os from 'node:os'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../../../../', import.meta.url))
const appRoot = fileURLToPath(new URL('../', import.meta.url))
const args = process.argv.slice(2), options = { origins: [] }
for (let index = 0; index < args.length; index += 2) {
  const name = args[index], value = args[index + 1]
  if (!value || !['--out-dir', '--version', '--allowed-host-origin', '--api-base'].includes(name)) throw new Error('未知构建参数或缺少值')
  if (name === '--allowed-host-origin') options.origins.push(value)
  else options[name.slice(2)] = value
}
const version = options.version || process.env.WORK_ORDERS_VERSION || '1.0.0'
if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(version) || version.includes('..')) throw new Error('版本号无效')
const origins = options.origins.length ? options.origins : (process.env.WORK_ORDERS_ALLOWED_HOST_ORIGINS || '').split(',').filter(Boolean)
if (!origins.length) throw new Error('请显式设置 WORK_ORDERS_ALLOWED_HOST_ORIGINS 或 --allowed-host-origin；构建不会猜测宿主来源')
function requireSecureRemote(url) {
  if (url.protocol === 'http:' && !['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)) {
    throw new Error('远程宿主与业务 API 必须使用 HTTPS；HTTP 仅允许 localhost、127.0.0.1 或 [::1] 回环地址')
  }
}
for (const origin of origins) {
  const url = new URL(origin)
  if (!['http:', 'https:'].includes(url.protocol) || url.origin !== origin) throw new Error('宿主白名单必须是精确 HTTP(S) origin')
  requireSecureRemote(url)
}
const apiBase = options['api-base'] || process.env.WORK_ORDERS_API_BASE || '/work-orders-api'
const apiUrl = new URL(apiBase, 'https://gateway.invalid')
if (!/^(?:\/(?!\/)|https?:\/\/)/.test(apiBase) || !['http:', 'https:'].includes(apiUrl.protocol) || apiUrl.username || apiUrl.password || apiUrl.search || apiUrl.hash || apiBase.startsWith('//')) throw new Error('业务 API 地址无效')
requireSecureRemote(apiUrl)
const destination = path.resolve(options['out-dir'] || path.join(appRoot, 'dist'))
const tempRelative = path.relative(path.resolve(os.tmpdir()), destination)
if (destination !== path.resolve(appRoot, 'dist') && (!tempRelative || path.isAbsolute(tempRelative) || tempRelative === '..' || tempRelative.startsWith('..' + path.sep))) {
  throw new Error('自定义输出须位于系统临时目录内；发布请打包生成的 dist')
}
if (existsSync(destination)) {
  let owned = false
  try { owned = JSON.parse(readFileSync(path.join(destination, 'build-info.json'), 'utf8')).application === 'work-orders' } catch {}
  if (!owned) throw new Error('输出目录不属于本应用构建，拒绝覆盖')
  rmSync(destination, { recursive: true, force: true })
}
mkdirSync(path.join(destination, 'assets'), { recursive: true })
const require = createRequire(import.meta.url)
let esbuild
try { esbuild = require('esbuild') } catch { esbuild = require(path.join(root, 'frontend/node_modules/esbuild')) }
await esbuild.build({ entryPoints: [path.join(appRoot, 'src/app.mjs')], outfile: path.join(destination, 'assets/app.js'),
  bundle: true, format: 'iife', platform: 'browser', target: 'es2022', minify: true, legalComments: 'eof' })
cpSync(path.join(appRoot, 'index.html'), path.join(destination, 'index.html'))
cpSync(path.join(appRoot, 'src/app.css'), path.join(destination, 'assets/app.css'))
cpSync(path.join(appRoot, 'vendor/vue.global.prod.js'), path.join(destination, 'assets/vue.global.prod.js'))
cpSync(path.join(appRoot, 'vendor/iframe-bridge.js'), path.join(destination, 'assets/iframe-bridge.js'))
cpSync(path.join(appRoot, 'vendor/vue.LICENSE.txt'), path.join(destination, 'assets/vue.LICENSE.txt'))
writeFileSync(path.join(destination, 'assets/config.js'), 'window.__WORK_ORDERS_CONFIG__ = Object.freeze(' +
  JSON.stringify({ version, allowedHostOrigins: origins, apiBase }) + ');\n')
writeFileSync(path.join(destination, 'build-info.json'), JSON.stringify({ application: 'work-orders', version, contractVersion: '2.0' }) + '\n')
console.log(`工单应用 ${version} 已构建：${destination}`)
