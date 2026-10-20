import { readFileSync, writeFileSync } from 'node:fs'
import { pathToFileURL } from 'node:url'
import { commonHttp, configuration, exactOrigin, knownFields, quote } from './gateway-policy.mjs'

export const applications = Object.freeze(['work-orders', 'stores'])

/** Public browser configuration only. An external static origin needs an
 * absolute API URL unless its operator supplies the matching API proxy. */
export function validateStaticInput(input) {
  knownFields(input, ['appCode', 'version', 'hostOrigin', 'apiBase'])
  if (!applications.includes(input.appCode)) throw new Error('Unknown business application')
  if (typeof input.version !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(input.version) || input.version.includes('..')) throw new Error('Invalid immutable application version')
  const host = exactOrigin(input.hostOrigin, 'host origin')
  const apiBase = input.apiBase
  if (typeof apiBase !== 'string' || /[\\\s;{}$"'?#]/.test(apiBase) || !/^(?:\/(?!\/)|https:\/\/)/.test(apiBase)) throw new Error('API base must be a root path or an absolute HTTPS URL')
  const api = new URL(apiBase, host.origin)
  if (api.protocol !== 'https:' || api.username || api.password || api.hash || api.search) throw new Error('Invalid public API base')
  if (apiBase.split('/').some(part => part === '.' || part === '..') || apiBase.includes('%')) throw new Error('API base must not contain encoded or relative segments')
  exactOrigin(api.origin, 'API origin')
  return { appCode: input.appCode, version: input.version, hostOrigin: host.origin, apiBase, apiOrigin: api.origin }
}

/** The edge removes the public version prefix. No SPA fallback is provided. */
export function renderBusinessStatic(input) {
  const publicConfig = validateStaticInput(input)
  const connects = [...new Set(["'self'", publicConfig.hostOrigin, publicConfig.apiOrigin])].join(' ')
  const csp = `default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src ${connects}; base-uri 'self'; object-src 'none'; frame-ancestors ${publicConfig.hostOrigin}`
  return configuration(commonHttp() + `    map $uri $business_file_cache {
        default "public, max-age=31536000, immutable";
        ~*\\.html$ "no-cache";
        ~/$ "no-cache";
    }
    map $status $business_response_cache {
        default "no-store";
        200 $business_file_cache;
        206 $business_file_cache;
        304 $business_file_cache;
    }
    server {
        listen 0.0.0.0:8080;
        server_name _;
        root /usr/share/nginx/html;
        index index.html;
        autoindex off;
        disable_symlinks on;
        add_header Cache-Control $business_response_cache always;
        add_header X-Content-Type-Options "nosniff" always;
        add_header Access-Control-Allow-Origin "*" always;
        add_header Content-Security-Policy ${quote(csp)} always;
        location = /healthz { add_header Cache-Control "no-store" always; return 200 "ok\\n"; }
        location ~ (?:^|/)\\. { return 404; }
        location ~* (?:^|/)(?:staging|incoming|uploads|backups|node_modules|private|secrets)(?:/|$) { return 404; }
        location / {
            limit_except GET { deny all; }
            try_files $uri $uri/index.html =404;
        }
    }
`)
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [source, output] = process.argv.slice(2)
  if (!source || !output || process.argv.length !== 4) throw new Error('Usage: node render-static.mjs <public-config.json> <nginx.conf>')
  writeFileSync(output, renderBusinessStatic(JSON.parse(readFileSync(source, 'utf8'))), { mode: 0o644 })
}
