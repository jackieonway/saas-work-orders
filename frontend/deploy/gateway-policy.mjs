import { isIP } from 'node:net'

export function knownFields(input, fields) {
  if (!input || typeof input !== 'object' || Array.isArray(input)) throw new Error('configuration must be an object')
  for (const field of Object.keys(input)) if (!fields.includes(field)) throw new Error(`unknown configuration field: ${field}`)
}

export function exactOrigin(value, label = 'public origin') {
  if (typeof value !== 'string' || /[\s;{}$"'\\]/.test(value)) throw new Error(`${label} must be an exact HTTPS origin`)
  let url
  try { url = new URL(value) } catch { throw new Error(`${label} must be an exact HTTPS origin`) }
  if (url.protocol !== 'https:' || url.username || url.password || value !== url.origin) throw new Error(`${label} must be an exact HTTPS origin without a trailing slash`)
  const ipv6 = url.hostname.startsWith('[') && isIP(url.hostname.slice(1, -1)) === 6
  const dns = url.hostname.length <= 253 && /^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)*\.?$/.test(url.hostname)
  if (!ipv6 && !dns) throw new Error(`${label} requires an exact hostname or IP without wildcards`)
  return url
}

export function origins(values = [], label = 'application origin') {
  if (!Array.isArray(values)) throw new Error(`${label} allowlist must be an array`)
  return [...new Set(values.map((value) => exactOrigin(value, label).origin))]
}

export function absolutePath(value, label) {
  if (typeof value !== 'string' || !/^(?:\/[A-Za-z0-9_ .:/-]*|[A-Za-z]:[\\/][A-Za-z0-9_ .:\\/-]*)$/.test(value)) throw new Error(`${label} must be a literal absolute path`)
  const normalized = value.replaceAll('\\', '/')
  if (normalized.split('/').some((segment) => segment === '..' || segment === '.') || normalized.includes('//')) throw new Error(`${label} must not contain relative path segments`)
  return normalized.replace(/\/$/, '') || '/'
}

export const quote = (value) => `"${value}"`

function privateIp(value) {
  const kind = isIP(value)
  if (kind === 4) {
    const [a, b] = value.split('.').map(Number)
    return a === 10 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || value === '127.0.0.1'
  }
  return kind === 6 && (value === '::1' || /^f[cd][0-9a-f]{2}:/i.test(value))
}

export function validateTrustedProxyIps(values = []) {
  if (!Array.isArray(values) || values.some((value) => typeof value !== 'string' || !privateIp(value))) throw new Error('trusted proxies must be exact private IP addresses; CIDRs and public addresses are forbidden')
  return [...new Set(values)]
}

function portNumber(value) {
  if (!/^[0-9]+$/.test(value) || Number(value) < 1 || Number(value) > 65535) throw new Error('invalid upstream/listener port')
  return Number(value)
}

export function address(value, label) {
  if (typeof value !== 'string') throw new Error(`${label} must be a host:port literal`)
  const match = /^(?:\[([0-9a-f:]+)\]|([A-Za-z0-9.-]+)):([0-9]+)$/i.exec(value)
  if (!match) throw new Error(`${label} must be a host:port literal without a URL or directives`)
  return { host: match[1] ?? match[2], port: portNumber(match[3]), value }
}

export function listener(value, { loopback = false } = {}) {
  const parsed = address(value, 'listener')
  if (!['0.0.0.0', '127.0.0.1', '::', '::1'].includes(parsed.host) || (loopback && !['127.0.0.1', '::1'].includes(parsed.host))) throw new Error('listener must bind an explicit local IP address')
  return parsed.value
}

export function upstreamPolicy({ allowedServices = ['backend', 'frontend-web', 'notification-ws', 'work-orders-api', 'stores-api', 'micro-static'], allowedUpstreamIps = [] } = {}) {
  if (!Array.isArray(allowedServices) || allowedServices.some((name) => typeof name !== 'string' || !/^[a-z][a-z0-9-]{0,62}$/.test(name))) throw new Error('upstream services must be explicit Docker service names')
  const ips = validateTrustedProxyIps(allowedUpstreamIps)
  return (value) => {
    const parsed = address(value, 'upstream')
    if (isIP(parsed.host)) {
      if (!privateIp(parsed.host) || (!['127.0.0.1', '::1'].includes(parsed.host) && !ips.includes(parsed.host))) throw new Error('upstream private IP must be explicitly allowlisted')
    } else if (!allowedServices.includes(parsed.host)) throw new Error('upstream service is not allowlisted')
    return parsed.value
  }
}

export function token(value, label) {
  if (typeof value !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(value)) throw new Error(`${label} must be a literal application name`)
  return value
}

export function version(value) {
  if (typeof value !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(value) || value.includes('..')) throw new Error('version must be a literal immutable path segment')
  return value
}

export function commonHttp({ mimeTypes = '/etc/nginx/mime.types', temporaryRoot = '/tmp', accessLog = '/dev/stdout' } = {}) {
  const temp = absolutePath(temporaryRoot, 'temporaryRoot')
  return `    include ${quote(absolutePath(mimeTypes, 'mimeTypes'))};
    types { application/javascript mjs cjs; }
    default_type application/octet-stream;
    log_format gateway '$remote_addr $request_method $uri $status $body_bytes_sent';
    access_log ${quote(absolutePath(accessLog, 'accessLog'))} gateway;
    server_tokens off;
    sendfile on;
    keepalive_timeout 65s;
    client_body_timeout 20s;
    client_header_timeout 15s;
    send_timeout 30s;
    client_max_body_size 2m;
    client_body_temp_path ${quote(`${temp}/client_body`)};
    proxy_temp_path ${quote(`${temp}/proxy`)};
    fastcgi_temp_path ${quote(`${temp}/fastcgi`)};
    uwsgi_temp_path ${quote(`${temp}/uwsgi`)};
    scgi_temp_path ${quote(`${temp}/scgi`)};
    map $http_upgrade $connection_upgrade { default upgrade; '' close; }
`
}

export function configuration(http, { pid = '/tmp/nginx.pid', errorLog = '/dev/stderr' } = {}) {
  return `# Generated from validated deployment JSON; no runtime environment substitution.
daemon off;
worker_processes auto;
pid ${quote(absolutePath(pid, 'pid'))};
error_log ${quote(absolutePath(errorLog, 'errorLog'))} warn;
events { worker_connections 1024; }
http {
${http}}
`
}

export function proxyHeaders(origin) {
  return `            proxy_set_header Host ${quote(origin.host)};
            proxy_set_header X-Real-IP $remote_addr;
            proxy_set_header X-Forwarded-For $remote_addr;
            proxy_set_header X-Forwarded-Host ${quote(origin.host)};
            proxy_set_header X-Forwarded-Proto https;
            proxy_set_header Forwarded "";
`
}

export function privateLocations() {
  return `        location = /api/internal { return 404; }
        location ^~ /api/internal/ { return 404; }
        location = /internal { return 404; }
        location ^~ /internal/ { return 404; }
        location = /work-orders-api/internal { return 404; }
        location ^~ /work-orders-api/internal/ { return 404; }
        location = /stores-api/internal { return 404; }
        location ^~ /stores-api/internal/ { return 404; }
        location = /notification-api/metrics { return 404; }
        location ^~ /notification-api/metrics/ { return 404; }
        location = /metrics { return 404; }
        location ^~ /metrics/ { return 404; }
        location = /work-orders-api/metrics { return 404; }
        location ^~ /work-orders-api/metrics/ { return 404; }
        location = /stores-api/metrics { return 404; }
        location ^~ /stores-api/metrics/ { return 404; }
        location = /socket.io/metrics { return 404; }
        location ^~ /socket.io/metrics/ { return 404; }
        location ~ ^/(?:@vite|@fs|@id|__vite|node_modules)(?:/|$) { return 404; }
`
}

export function apiLocations(origin, upstreams) {
  const location = (prefix, target, { socket = false, release = false } = {}) => {
    if (!target) return `        location = /${prefix} { return 404; }\n        location /${prefix}/ { return 404; }\n`
    const directives = `            set $${target.name} ${quote(target.address)};
            proxy_http_version 1.1;
${proxyHeaders(origin)}${socket ? `            proxy_set_header Upgrade $http_upgrade;
            proxy_set_header Connection $connection_upgrade;
            proxy_buffering off;
            proxy_read_timeout 75s;
` : `            proxy_set_header Connection "";
            proxy_read_timeout ${release ? '65s' : '30s'};
`}
            proxy_connect_timeout 5s;
            proxy_send_timeout ${release ? '65s' : '30s'};
            proxy_hide_header Cache-Control;
            add_header Cache-Control "no-store" always;
            add_header X-Content-Type-Options "nosniff" always;
            proxy_pass http://$${target.name}$request_uri;
`
    if (release) return `        location ~ ^/api/applications/[0-9a-f-]+/(?:releases(?:/upload|/[0-9a-f-]+/(?:preflight|activate))?|deployments/[a-z0-9_-]+/rollback)$ {
            client_max_body_size 20m;
${directives}        }
`
    return `        location = /${prefix} {
${directives}        }
        location /${prefix}/ {
${directives}        }
`
  }
  return location('api', upstreams.backend, { release: true }) + location('api', upstreams.backend) + location('oauth', upstreams.backend) + location('work-orders-api', upstreams.workOrders) + location('stores-api', upstreams.stores) + location('socket.io', upstreams.notification, { socket: true })
}

export function staticMaps() {
  return `    map $uri $version_file_cache {
        default "public, max-age=31536000, immutable";
        ~*\\.html$ "no-cache";
        ~/$ "no-cache";
    }
    map $status $version_response_cache {
        default "no-store";
        200 $version_file_cache;
        206 $version_file_cache;
        304 $version_file_cache;
    }
    map $uri $host_file_cache {
        default "no-cache";
        ~^/assets/ "public, max-age=31536000, immutable";
        ~*(?:^|/)(?:runtime-config|config)\\.json$ "no-store";
    }
    map $status $host_response_cache {
        default "no-store";
        200 $host_file_cache;
        206 $host_file_cache;
        304 $host_file_cache;
    }
    map $http_accept $host_accept_html { default 0; ~*text/html 1; }
`
}

export function contentSecurityPolicy({ origin, applicationOrigins = [], imageOrigins = [], trustedHtmlEval = false, micro = false }) {
  const applications = origins(applicationOrigins)
  if (typeof trustedHtmlEval !== 'boolean') throw new Error('trustedHtmlEval must be an explicit boolean')
  const source = ["'self'", ...applications].join(' ')
  const images = ["'self'", 'data:', 'blob:', ...origins(imageOrigins, 'image origin')].join(' ')
  return `default-src 'none'; script-src 'self'${trustedHtmlEval ? " 'unsafe-eval'" : ''}; style-src 'self' 'unsafe-inline'; img-src ${images}; font-src 'self' data:; connect-src ${source} ${origin.origin} ${origin.origin.replace('https:', 'wss:')}; frame-src ${source}; base-uri 'self'; object-src 'none'; frame-ancestors ${micro ? origin.origin : "'none'"}`
}

export function staticHeaders({ origin, applicationOrigins = [], imageOrigins = [], trustedHtmlEval = false, micro = false, cache = micro ? '$version_response_cache' : '$host_response_cache' }) {
  const csp = contentSecurityPolicy({ origin, applicationOrigins, imageOrigins, trustedHtmlEval, micro })
  return `            add_header Cache-Control ${cache} always;
            add_header X-Content-Type-Options "nosniff" always;
            add_header Content-Security-Policy ${quote(csp)} always;
${micro ? '            add_header Access-Control-Allow-Origin "*" always;\n' : '            add_header X-Frame-Options "DENY" always;\n'}`
}

export function zipLocations({ origin, microRoot, applicationOrigins, imageOrigins, trustedHtmlEval }) {
  const headers = staticHeaders({ origin, applicationOrigins, imageOrigins, trustedHtmlEval, micro: true })
  return `        location = /sub-apps { return 404; }
        location ~ (?:^|/)\\. { return 404; }
        location ~* (?:^|/)release\\.json(?:/|$) { return 404; }
        location ~* (?:^|/)(?:staging|incoming|uploads|backups|tmp)(?:/|$) { return 404; }
        location ~ ^/sub-apps/[A-Za-z0-9][A-Za-z0-9_-]*/versions/[A-Za-z0-9][A-Za-z0-9._-]*/ {
            rewrite ^/sub-apps/(.*)$ /$1 break;
            root ${quote(absolutePath(microRoot, 'microRoot'))};
            index index.html;
            autoindex off;
            disable_symlinks on;
            try_files $uri $uri/index.html =404;
            limit_except GET { deny all; }
${headers}        }
        location /sub-apps/ { return 404; }
`
}
