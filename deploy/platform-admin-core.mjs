import { readFileSync, statSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import https from 'node:https';
import { grantPlan, grantSql } from './grants.mjs';

const MAX_JSON_BYTES = 1024 * 1024;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const ENVIRONMENT = /^[a-z0-9][a-z0-9_-]{0,31}$/;
const VERSION = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
const manifest = JSON.parse(readFileSync(new URL('./bootstrap.manifest.json', import.meta.url), 'utf8'));

export class SafeAdminError extends Error {
  constructor(category, status) {
    super(category);
    this.category = category;
    this.status = status;
  }
}

const fail = (category, status) => { throw new SafeAdminError(category, status); };
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const uuid = value => typeof value === 'string' && UUID.test(value) ? value : fail('INVALID_ID');
const environment = value => typeof value === 'string' && ENVIRONMENT.test(value) ? value : fail('INVALID_ENVIRONMENT');
const version = value => typeof value === 'string' && VERSION.test(value) && !value.includes('..') ? value : fail('INVALID_VERSION');
function keys(value, allowed, category = 'INVALID_CONTRACT') {
  if (!object(value) || Object.keys(value).some(key => !allowed.includes(key))) fail(category);
}

function guardBootstrapDefinition() {
  const app = manifest.application;
  const permissions = ['ticket:read', 'ticket:create', 'ticket:update', 'ticket:delete', 'ticket:export', 'notification:publish'];
  keys(app, ['code', 'name', 'icon', 'basePath', 'entryType', 'entryUrl', 'runtimeMode', 'autoCreateMenu', 'description', 'sortOrder', 'status'], 'INVALID_BOOTSTRAP_MANIFEST');
  if (manifest.schemaVersion !== '1.0' || app.code !== 'work-orders' || app.runtimeMode !== 'IFRAME' || app.entryType !== 'HTML' || app.basePath !== '/micro/work-orders' || app.entryUrl !== '/sub-apps/work-orders/unreleased/index.html' || app.autoCreateMenu !== false || app.status !== 'ENABLED' || !Array.isArray(manifest.grants) || manifest.grants.length || manifest.menus.length !== 7) fail('INVALID_BOOTSTRAP_MANIFEST');
  for (let i = 0; i < manifest.menus.length; i++) {
    const row = manifest.menus[i];
    keys(row, ['key', 'parentKey', 'type', 'title', 'path', 'permission', 'sortOrder', 'visible', 'linkTarget'], 'INVALID_BOOTSTRAP_MANIFEST');
    if (i === 0) {
      if (row.key !== 'root' || row.parentKey !== null || row.type !== 'MENU' || row.path !== '/micro/work-orders' || row.permission !== '' || row.visible !== true || row.linkTarget !== 'MICRO') fail('INVALID_BOOTSTRAP_MANIFEST');
    } else if (row.key !== permissions[i - 1] || row.permission !== permissions[i - 1] || row.parentKey !== 'root' || row.type !== 'BUTTON' || row.path !== '' || row.visible !== false || row.linkTarget !== 'BLANK') fail('INVALID_BOOTSTRAP_MANIFEST');
  }
}

export const usage = `work-orders 平台管理员工具（Node 24，无第三方依赖）
用法: node business-apps/work-orders/deploy/platform-admin.mjs [command] [options]

默认 command=plan，仅显示本地计划；任何远程写入都必须加 --apply。
  plan                         本地 bootstrap 计划，无网络
  inspect                      核对应用、指定机密客户端与本应用菜单
  bootstrap [--apply]          创建缺少的应用/菜单，关联指定未绑定客户端
  grants-plan --manifest FILE [--sql-out FILE]
                               生成显式授权人工计划；SQL 草案默认 ROLLBACK
  register --release-file FILE [--apply]
                               登记 package-microapp 产出的 release.json
  releases                     查询本应用发布版本的安全摘要
  preflight --release-id UUID [--apply]
  activate --release-id UUID [--apply]
  rollback [--apply]
  deployments                  查询本应用部署指针

共同选项: --environment NAME（默认 production） --app-id UUID
开发回环 HTTP 额外要求: --environment development --allow-loopback-http
远程环境读取 PLATFORM_API_ORIGIN、PLATFORM_ADMIN_BEARER。
inspect/bootstrap 另读取 PLATFORM_CLIENT_ID（平台生成的实际客户端 ID）。
客户端 secret 仅业务服务从环境读取；本工具不接收、不生成、不输出 secret。
不自动授予租户/组织/角色/成员权限，不创建 TRUSTED，不自动预检/启用/回滚。`;

export function parseArgs(args) {
  if (args.length === 1 && args[0] === '--help') return { command: 'help' };
  const commands = new Set(['plan', 'inspect', 'bootstrap', 'grants-plan', 'register', 'releases', 'preflight', 'activate', 'rollback', 'deployments']);
  const command = args[0] && !args[0].startsWith('--') ? args[0] : 'plan';
  if (!commands.has(command)) fail('INVALID_ARGUMENT');
  const result = { command, environment: 'production', apply: false, allowLoopbackHttp: false };
  const allowed = new Set(['environment', 'app-id', 'release-id', 'release-file', 'manifest', 'sql-out', 'apply', 'allow-loopback-http']);
  const seen = new Set();
  for (let i = command === args[0] ? 1 : 0; i < args.length; i++) {
    const name = args[i].startsWith('--') ? args[i].slice(2) : '';
    if (!allowed.has(name) || seen.has(name)) fail('INVALID_ARGUMENT');
    seen.add(name);
    if (name === 'apply') result.apply = true;
    else if (name === 'allow-loopback-http') result.allowLoopbackHttp = true;
    else {
      const value = args[++i];
      if (!value || value.startsWith('--')) fail('INVALID_ARGUMENT');
      result[name] = value;
    }
  }
  environment(result.environment);
  if (result['app-id']) uuid(result['app-id']);
  if (result['release-id']) uuid(result['release-id']);
  const specific = { 'release-id': ['preflight', 'activate'], 'release-file': ['register'], manifest: ['grants-plan'], 'sql-out': ['grants-plan'] };
  for (const [name, commandsForFlag] of Object.entries(specific)) {
    if (result[name] && !commandsForFlag.includes(command)) fail('INVALID_ARGUMENT');
  }
  if (['preflight', 'activate'].includes(command) && !result['release-id']) fail('INVALID_ARGUMENT');
  if (command === 'register' && !result['release-file']) fail('INVALID_ARGUMENT');
  if (command === 'grants-plan' && !result.manifest) fail('INVALID_ARGUMENT');
  if (result.apply && ['plan', 'inspect', 'grants-plan', 'releases', 'deployments'].includes(command)) fail('INVALID_ARGUMENT');
  return result;
}

function readJson(path, category) {
  try {
    if (!statSync(path).isFile() || statSync(path).size > MAX_JSON_BYTES) fail(category);
    return JSON.parse(readFileSync(path, 'utf8'));
  } catch { fail(category); }
}

export function adminConfig(env, options) {
  let origin;
  try { origin = new URL(env.PLATFORM_API_ORIGIN); } catch { fail('INVALID_ORIGIN'); }
  if (!['http:', 'https:'].includes(origin.protocol) || origin.username || origin.password || origin.search || origin.hash || origin.pathname !== '/') fail('INVALID_ORIGIN');
  if (typeof env.PLATFORM_API_ORIGIN !== 'string' || /\s/.test(env.PLATFORM_API_ORIGIN)) fail('INVALID_ORIGIN');
  const loopback = ['127.0.0.1', '[::1]', 'localhost'].includes(origin.hostname);
  if (origin.protocol !== 'https:' && !(options.environment === 'development' && options.allowLoopbackHttp && loopback)) fail('HTTPS_REQUIRED');
  const bearer = env.PLATFORM_ADMIN_BEARER;
  if (typeof bearer !== 'string' || !bearer || bearer.length > 8192 || /[\s\x00-\x1f\x7f]/.test(bearer)) fail('ADMIN_BEARER_REQUIRED');
  return { origin: origin.origin, bearer };
}

/** Direct native HTTP avoids ambient proxy routing. Redirects and retries are forbidden. */
export class ManagementApi {
  constructor(config, { timeoutMs = 35_000, maxBytes = MAX_JSON_BYTES } = {}) {
    this.config = config;
    this.timeoutMs = timeoutMs;
    this.maxBytes = maxBytes;
  }
  async request(method, path, body) {
    if (!['GET', 'POST', 'PUT'].includes(method) || !path.startsWith('/api/') || path.startsWith('//') || /[\r\n\\#]/.test(path)) fail('INVALID_REQUEST');
    const target = new URL(path, this.config.origin);
    if (target.origin !== this.config.origin) fail('INVALID_REQUEST');
    const content = body === undefined ? null : Buffer.from(JSON.stringify(body));
    return new Promise((resolve, reject) => {
      let settled = false;
      let timer;
      const finish = (error, value) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        if (error) reject(error); else resolve(value);
      };
      const transport = target.protocol === 'https:' ? https : http;
      const request = transport.request(target, {
        method,
        agent: false,
        rejectUnauthorized: true,
        headers: {
          authorization: `Bearer ${this.config.bearer}`,
          accept: 'application/json',
          ...(content ? { 'content-type': 'application/json', 'content-length': content.length } : {}),
        },
      }, response => {
        const status = response.statusCode ?? 0;
        if (status >= 300 && status < 400) {
          response.destroy();
          finish(new SafeAdminError('REDIRECT_REJECTED', status));
          return;
        }
        if (status < 200 || status >= 300) {
          response.destroy();
          finish(new SafeAdminError('HTTP_ERROR', status));
          return;
        }
        const type = response.headers['content-type'] ?? '';
        if (!/^application\/(?:json|[\w.+-]+\+json)(?:\s*;|$)/i.test(type)) {
          response.destroy();
          finish(new SafeAdminError('INVALID_RESPONSE', status));
          return;
        }
        const chunks = [];
        let bytes = 0;
        response.on('data', chunk => {
          bytes += chunk.length;
          if (bytes > this.maxBytes) {
            response.destroy();
            finish(new SafeAdminError('RESPONSE_TOO_LARGE', status));
          } else chunks.push(chunk);
        });
        response.on('error', () => finish(new SafeAdminError('NETWORK_ERROR')));
        response.on('end', () => {
          if (settled) return;
          try { finish(null, JSON.parse(Buffer.concat(chunks).toString('utf8'))); }
          catch { finish(new SafeAdminError('INVALID_RESPONSE', status)); }
        });
      });
      request.on('error', () => finish(new SafeAdminError('NETWORK_ERROR')));
      timer = setTimeout(() => {
        finish(new SafeAdminError('REQUEST_TIMEOUT'));
        request.destroy();
      }, this.timeoutMs);
      request.end(content);
    });
  }
}

function verifyApp(row, expectedId) {
  if (!object(row) || row.code !== 'work-orders' || !UUID.test(row.id ?? '') || (expectedId && row.id !== expectedId)) fail('APPLICATION_CONFLICT');
  if (row.runtimeMode !== 'IFRAME' || row.entryType !== 'HTML' || row.basePath !== '/micro/work-orders' || !['ENABLED', 'DISABLED'].includes(row.status)) fail('APPLICATION_CONFLICT');
  return row;
}

async function findApp(api, expectedId) {
  const rows = await api.request('GET', '/api/applications');
  if (!Array.isArray(rows)) fail('INVALID_CONTRACT');
  const own = rows.filter(row => row?.code === 'work-orders');
  if (own.length > 1) fail('APPLICATION_CONFLICT');
  if (!own.length && expectedId) fail('APPLICATION_NOT_FOUND');
  return own.length ? verifyApp(own[0], expectedId) : null;
}

function verifyClient(row, appId) {
  if (!object(row) || !UUID.test(row.id ?? '') || row.clientName !== manifest.oauthClient.clientName || row.clientType !== 'CONFIDENTIAL' || row.status !== 'ENABLED') fail('CLIENT_CONFLICT');
  if (row.applicationId !== null && !UUID.test(row.applicationId ?? '')) fail('CLIENT_CONFLICT');
  for (const field of ['grantTypes', 'scopes', 'redirectUris']) {
    if (!Array.isArray(row[field]) || JSON.stringify([...row[field]].sort()) !== JSON.stringify([...manifest.oauthClient[field]].sort())) fail('CLIENT_CONFLICT');
  }
  if (row.accessTokenValidity !== 300 || row.refreshTokenValidity !== 300) fail('CLIENT_CONFLICT');
  if (row.applicationId !== null && row.applicationId !== appId) fail('CLIENT_OWNERSHIP_CONFLICT');
  return row;
}

async function findClient(api, env, app) {
  const clientId = env.PLATFORM_CLIENT_ID;
  if (typeof clientId !== 'string' || !clientId || clientId.length > 128 || /[\s\x00-\x1f\x7f]/.test(clientId)) fail('CLIENT_ID_REQUIRED');
  const rows = await api.request('GET', '/api/oauth/clients');
  if (!Array.isArray(rows)) fail('INVALID_CONTRACT');
  const matching = rows.filter(row => row?.clientId === clientId);
  const named = rows.filter(row => row?.clientName === manifest.oauthClient.clientName);
  if (matching.length !== 1 || named.length !== 1 || named[0] !== matching[0]) fail('CLIENT_CONFLICT');
  const client = verifyClient(matching[0], app?.id);
  if (app && rows.some(row => row.id !== client.id && row.applicationId === app.id)) fail('CLIENT_OWNERSHIP_CONFLICT');
  return client;
}

function flattenMenus(tree, appId) {
  if (!object(tree) || !Array.isArray(tree.menus)) fail('INVALID_CONTRACT');
  const flat = [];
  const seen = new Set();
  function visit(rows, depth = 0) {
    if (depth > 32 || flat.length > 1024) fail('INVALID_CONTRACT');
    for (const row of rows) {
      if (!object(row) || !UUID.test(row.id ?? '') || row.applicationId !== appId || seen.has(row.id) || (row.parentId !== null && !UUID.test(row.parentId ?? ''))) fail('MENU_OWNERSHIP_CONFLICT');
      seen.add(row.id);
      flat.push(row);
      if (row.children !== undefined) {
        if (!Array.isArray(row.children)) fail('INVALID_CONTRACT');
        visit(row.children, depth + 1);
      }
    }
  }
  visit(tree.menus);
  return flat;
}

function existingMenu(rows, desired, rootId) {
  const candidates = rows.filter(row => desired.key === 'root' ? row.path === desired.path : row.permission === desired.permission);
  if (candidates.length > 1) fail('MENU_CONFLICT');
  if (!candidates.length) return null;
  const row = candidates[0];
  if (row.type !== desired.type || row.parentId !== (desired.parentKey ? rootId : null) || row.path !== desired.path || row.permission !== desired.permission || row.visible !== desired.visible || row.linkTarget !== desired.linkTarget || row.linkUrl !== '') fail('MENU_CONFLICT');
  return row;
}

function checkMenus(rows) {
  const root = existingMenu(rows, manifest.menus[0], null);
  const ids = {};
  if (root) ids.root = root.id;
  for (const desired of manifest.menus.slice(1)) {
    const row = existingMenu(rows, desired, root?.id);
    if (row) ids[desired.key] = row.id;
  }
  return ids;
}

async function inspectBootstrap(api, env, options) {
  const app = await findApp(api, options['app-id']);
  // Validate the explicitly selected existing client before creating an application.
  const client = await findClient(api, env, app);
  const rows = app ? flattenMenus(await api.request('GET', `/api/menus/tree?applicationId=${app.id}`), app.id) : [];
  return { app, client, rows, menuIds: checkMenus(rows) };
}

function plan(command, options) {
  return { ok: true, mode: 'PLAN', command, applicationCode: 'work-orders', runtimeMode: 'IFRAME', entryType: 'HTML', environment: options.environment, externalWrites: false, permissions: manifest.menus.slice(1).map(row => row.permission), grants: [], clientProvisioning: 'MANUAL_PLATFORM_UI', activation: 'SEPARATE_EXPLICIT_COMMAND' };
}

async function bootstrap(api, env, options) {
  const inspected = await inspectBootstrap(api, env, options);
  let { app, client, rows, menuIds } = inspected;
  const created = [];
  if (!app) {
    app = verifyApp(await api.request('POST', '/api/applications', manifest.application));
    created.push('application');
  }
  if (client.applicationId === null) {
    // The API has no CAS. Recheck narrows the window; operators still serialize admin runs.
    client = await findClient(api, env, app);
    if (client.applicationId !== null && client.applicationId !== app.id) fail('CLIENT_OWNERSHIP_CONFLICT');
  }
  if (client.applicationId === null) {
    const response = await api.request('PUT', `/api/oauth/clients/${client.id}`, { applicationId: app.id });
    client = verifyClient(response, app.id);
    if (client.applicationId !== app.id || client.clientId !== env.PLATFORM_CLIENT_ID) fail('CLIENT_OWNERSHIP_CONFLICT');
    created.push('clientAssociation');
  }
  for (const desired of manifest.menus) {
    let row = existingMenu(rows, desired, menuIds.root);
    if (!row) {
      const { key, parentKey, ...body } = desired;
      row = await api.request('POST', '/api/menus', { ...body, applicationId: app.id, parentId: parentKey ? menuIds.root : null, linkUrl: '', icon: '', component: '' });
      if (!object(row) || !UUID.test(row.id ?? '') || row.applicationId !== app.id) fail('MENU_OWNERSHIP_CONFLICT');
      if (rows.some(existing => existing.id === row.id)) fail('MENU_CONFLICT');
      rows.push(row);
      if (existingMenu(rows, desired, menuIds.root)?.id !== row.id) fail('MENU_CONFLICT');
      created.push(key);
    }
    menuIds[desired.key] = row.id;
  }
  return { ok: true, mode: 'APPLIED', command: 'bootstrap', applicationId: app.id, clientRecordId: client.id, menuIds, created, grants: [], releaseActivated: false };
}

function safeUrl(raw, base, options) {
  if (typeof raw !== 'string' || !raw || /[\s\\\x00-\x1f\x7f]/.test(raw) || raw.startsWith('//')) fail('INVALID_RELEASE');
  let url;
  try { url = new URL(raw, base); } catch { fail('INVALID_RELEASE'); }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) fail('INVALID_RELEASE');
  for (const segment of url.pathname.slice(1).split('/')) {
    let decoded;
    try { decoded = decodeURIComponent(segment); } catch { fail('INVALID_RELEASE'); }
    // A gateway may decode escapes before routing. Reject encoded separators and double encoding.
    if (decoded === '.' || decoded === '..' || /[\/\\%\x00-\x1f\x7f]/.test(decoded)) fail('INVALID_RELEASE');
  }
  if (url.protocol !== 'https:' && !(options.environment === 'development' && options.allowLoopbackHttp && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname))) fail('HTTPS_REQUIRED');
  return url;
}

export function validateRelease(value, options) {
  keys(value, ['environment', 'version', 'entryType', 'entryUrl', 'healthUrl', 'resources'], 'INVALID_RELEASE');
  if (environment(value.environment) !== options.environment || value.entryType !== 'HTML') fail('INVALID_RELEASE');
  version(value.version);
  // A synthetic HTTPS origin validates relative artifacts without any network configuration.
  const entry = safeUrl(value.entryUrl, 'https://release.invalid', options);
  const expectedPrefix = `/sub-apps/work-orders/versions/${value.version}/`;
  if (!entry.pathname.startsWith(expectedPrefix) || !/\.html?$/.test(entry.pathname)) fail('INVALID_RELEASE');
  if (value.healthUrl !== undefined && value.healthUrl !== null) safeUrl(value.healthUrl, entry.origin, options);
  if (!Array.isArray(value.resources) || value.resources.length > 48) fail('INVALID_RELEASE');
  const seen = new Set();
  for (const resource of value.resources) {
    keys(resource, ['url', 'kind'], 'INVALID_RELEASE');
    if (!['SCRIPT', 'STYLE', 'ASSET'].includes(resource.kind)) fail('INVALID_RELEASE');
    const url = safeUrl(resource.url, entry, options);
    if (!url.pathname.startsWith(expectedPrefix) || url.origin !== entry.origin || url.href === entry.href || seen.has(url.href)) fail('INVALID_RELEASE');
    seen.add(url.href);
  }
  return value;
}

function releaseSummary(row, appId, env) {
  if (!object(row) || !UUID.test(row.id ?? '') || row.applicationId !== appId || row.environment !== env || row.entryType !== 'HTML' || !['DRAFT', 'PASSED', 'FAILED'].includes(row.validationStatus)) fail('INVALID_CONTRACT');
  return { id: row.id, environment: environment(row.environment), version: version(row.version), validationStatus: row.validationStatus, active: row.active === true };
}

async function releaseRows(api, app, env) {
  const rows = await api.request('GET', `/api/applications/${app.id}/releases?environment=${env}`);
  if (!Array.isArray(rows)) fail('INVALID_CONTRACT');
  return rows.map(row => releaseSummary(row, app.id, env));
}

function deploymentSummary(row, appId) {
  if (!object(row) || row.applicationId !== appId || row.entryType !== 'HTML') fail('INVALID_CONTRACT');
  return { applicationId: appId, environment: environment(row.environment), activeReleaseId: uuid(row.activeReleaseId), previousReleaseId: row.previousReleaseId === null ? null : uuid(row.previousReleaseId), version: version(row.version) };
}

async function releaseCommand(api, options, artifact) {
  const app = await findApp(api, options['app-id']);
  if (!app) fail('APPLICATION_NOT_FOUND');
  const base = `/api/applications/${app.id}`;
  const env = options.environment;
  if (options.command === 'deployments' || options.command === 'rollback') {
    const rows = await api.request('GET', `${base}/deployments`);
    if (!Array.isArray(rows)) fail('INVALID_CONTRACT');
    const summaries = rows.map(row => deploymentSummary(row, app.id)).filter(row => row.environment === env);
    if (options.command === 'deployments') return { ok: true, command: 'deployments', deployments: summaries };
    if (summaries.length !== 1 || summaries[0].previousReleaseId === null) fail('ROLLBACK_UNAVAILABLE');
    const response = await api.request('POST', `${base}/deployments/${env}/rollback`);
    const summary = deploymentSummary(response, app.id);
    if (summary.environment !== env || summary.activeReleaseId !== summaries[0].previousReleaseId || summary.previousReleaseId !== summaries[0].activeReleaseId) fail('DEPLOYMENT_CONFLICT');
    return { ok: true, mode: 'APPLIED', command: 'rollback', deployment: summary };
  }
  const rows = await releaseRows(api, app, env);
  if (options.command === 'releases') return { ok: true, command: 'releases', applicationId: app.id, releases: rows };
  if (options.command === 'register') {
    if (rows.some(row => row.version === artifact.version)) fail('VERSION_EXISTS');
    const row = releaseSummary(await api.request('POST', `${base}/releases`, artifact), app.id, env);
    if (row.version !== artifact.version || row.validationStatus !== 'DRAFT') fail('INVALID_CONTRACT');
    return { ok: true, mode: 'APPLIED', command: 'register', release: row, releaseActivated: false };
  }
  const target = rows.find(row => row.id === options['release-id']);
  if (!target) fail('RELEASE_NOT_FOUND');
  if (options.command === 'activate' && target.validationStatus !== 'PASSED') fail('PREFLIGHT_REQUIRED');
  const response = await api.request('POST', `${base}/releases/${target.id}/${options.command}`);
  if (options.command === 'preflight') {
    const row = releaseSummary(response, app.id, env);
    if (row.id !== target.id) fail('INVALID_CONTRACT');
    if (row.validationStatus !== 'PASSED') fail('PREFLIGHT_FAILED');
    return { ok: true, mode: 'APPLIED', command: 'preflight', release: row, releaseActivated: false };
  }
  const deployment = deploymentSummary(response, app.id);
  if (deployment.environment !== env || deployment.activeReleaseId !== target.id || deployment.version !== target.version) fail('DEPLOYMENT_CONFLICT');
  return { ok: true, mode: 'APPLIED', command: 'activate', deployment };
}

export async function run(args, env, injectedApi) {
  const options = parseArgs(args);
  if (options.command === 'help') return usage;
  guardBootstrapDefinition();
  if (options.command === 'grants-plan') {
    const value = readJson(options.manifest, 'INVALID_GRANT_MANIFEST');
    let result;
    let sql;
    try { result = grantPlan(value); sql = grantSql(value); } catch { fail('INVALID_GRANT_MANIFEST'); }
    if (options['sql-out']) {
      try { writeFileSync(options['sql-out'], sql, { flag: 'wx', mode: 0o600 }); }
      catch { fail('SQL_OUTPUT_FAILED'); }
    }
    return { ok: true, mode: 'MANUAL_REVIEW', ...result, sqlWritten: Boolean(options['sql-out']), externalWrites: false };
  }
  const artifact = options.command === 'register' ? validateRelease(readJson(options['release-file'], 'INVALID_RELEASE'), options) : undefined;
  if (options.command === 'plan' || (!options.apply && !['inspect', 'releases', 'deployments'].includes(options.command))) {
    const result = plan(options.command, options);
    if (artifact) result.version = artifact.version;
    if (options['release-id']) result.releaseId = options['release-id'];
    return result;
  }
  const api = injectedApi ?? new ManagementApi(adminConfig(env, options));
  if (options.command === 'inspect') {
    const { app, client, menuIds } = await inspectBootstrap(api, env, options);
    return { ok: true, mode: 'READ_ONLY', applicationId: app?.id ?? null, clientRecordId: client.id, clientAssociated: Boolean(app && client.applicationId === app.id), applicationStatus: app?.status ?? null, menuIds, missingMenus: manifest.menus.filter(row => !menuIds[row.key]).map(row => row.key), grants: [] };
  }
  if (options.command === 'bootstrap') return bootstrap(api, env, options);
  return releaseCommand(api, options, artifact);
}
