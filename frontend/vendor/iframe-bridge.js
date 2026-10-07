/* Classic script: opaque-origin sandbox frames can load this without module CORS. */
(function (global) {
  'use strict'
  var VERSION = '2.0'
  var variants = ['default', 'success', 'error', 'warning', 'info']
  function abortError() { return new DOMException('Application context cancelled or expired', 'AbortError') }
  function connect(options) {
    if (!options || typeof options.appCode !== 'string' || !options.appCode ||
        !Array.isArray(options.allowedHostOrigins) || options.allowedHostOrigins.length === 0 ||
        !options.lifecycle || typeof options.lifecycle.mount !== 'function' || typeof options.lifecycle.unmount !== 'function') {
      throw new Error('DanteMicroIframe requires appCode, explicit allowedHostOrigins and mount/unmount lifecycle')
    }
    var origins = options.allowedHostOrigins.map(function (origin) {
      var url = new URL(origin)
      if (!/^https?:$/.test(url.protocol) || origin !== url.origin) throw new Error('allowedHostOrigins must contain exact HTTP(S) origins')
      return origin
    })
    var lifecycle = options.lifecycle
    var container = options.container || document.body
    var port = null, nonce = null, context = null, sequence = 0, mounted = false, stopped = false
    var expiryTimer = null, chain = Promise.resolve()
    var nextDownload = 0, pendingDownloads = new Map()
    function settleDownload(id, error) {
      var pending = pendingDownloads.get(id)
      if (!pending) return
      pendingDownloads.delete(id)
      clearTimeout(pending.timer)
      pending.snapshot.signal.removeEventListener('abort', pending.abort)
      if (error) pending.reject(error)
      else pending.resolve()
    }
    function usable(snapshot) {
      return !stopped && context === snapshot && snapshot && !snapshot.signal.aborted &&
        Date.parse(snapshot.expiresAt) > Date.now() && Date.parse(snapshot.tokenExpiresAt) > Date.now()
    }
    function send(kind, epoch, extra) {
      if (port && !stopped) port.postMessage(Object.assign({ kind: kind, nonce: nonce, contextEpoch: epoch }, extra))
    }
    function abortContext() {
      if (context) context.controller.abort()
      pendingDownloads.forEach(function (_, id) { settleDownload(id, abortError()) })
      if (expiryTimer !== null) clearTimeout(expiryTimer)
      expiryTimer = null
    }
    function dispose() {
      if (stopped) return
      stopped = true
      sequence++
      abortContext()
      context = null
      global.removeEventListener('message', receiveConnect)
      global.removeEventListener('pagehide', dispose)
      if (port) { port.onmessage = null; port.onmessageerror = null; port.close(); port = null }
      chain = chain.catch(function () {}).then(function () {
        if (mounted) { mounted = false; return lifecycle.unmount() }
      }).catch(function () {})
    }
    function hydrate(dto) {
      var expiry = dto && Math.min(Date.parse(dto.expiresAt), Date.parse(dto.tokenExpiresAt))
      if (!dto || dto.contractVersion !== VERSION || dto.appCode !== options.appCode ||
          dto.basePath !== '/micro/' + options.appCode || !Number.isSafeInteger(dto.contextEpoch) || dto.contextEpoch <= 0 ||
          !dto.contextVersion || typeof dto.token !== 'string' || !dto.token || !Number.isFinite(expiry) || expiry <= Date.now() ||
          !dto.user || !dto.user.id || !dto.tenant || !dto.tenant.id || !dto.organization || !dto.organization.id ||
          (context && dto.contextEpoch < context.contextEpoch)) return null
      // 立即取消旧 context，而不等待慢 mount/update；队列仅串行运行生命周期。
      abortContext()
      var controller = new AbortController()
      var snapshot = Object.assign({}, dto, { signal: controller.signal, controller: controller })
      snapshot.onNavigate = function (path) {
        if (usable(snapshot) && typeof path === 'string') send('DANTE_MICRO_NAVIGATE', snapshot.contextEpoch, { path: path })
      }
      snapshot.onNotify = function (message, variant) {
        variant = variant === undefined ? 'default' : variant
        if (usable(snapshot) && typeof message === 'string' && message.length > 0 && message.length <= 1000 && variants.indexOf(variant) >= 0) {
          send('DANTE_MICRO_NOTIFY', snapshot.contextEpoch, { message: message, variant: variant })
        }
      }
      context = snapshot
      expiryTimer = setTimeout(function () { if (context === snapshot) abortContext() }, Math.max(1, expiry - Date.now()))
      return snapshot
    }
    function receive(event) {
      var data = event.data
      if (stopped || !data || typeof data !== 'object' || data.nonce !== nonce || !Number.isSafeInteger(data.contextEpoch)) return
      if (data.kind === 'DANTE_MICRO_DOWNLOAD_RESULT') {
        var pending = pendingDownloads.get(data.requestId)
        if (pending && usable(pending.snapshot) && data.contextEpoch === pending.snapshot.contextEpoch) {
          settleDownload(data.requestId, data.ok === true ? undefined : new Error('CSV download denied'))
        }
        return
      }
      if (data.kind === 'DANTE_MICRO_UNMOUNT') {
        if (!context || data.contextEpoch === context.contextEpoch) dispose()
        return
      }
      if (data.kind === 'DANTE_MICRO_CONTEXT_ABORT') {
        if (context && data.contextEpoch === context.contextEpoch) { sequence++; abortContext() }
        return
      }
      if ((data.kind !== 'DANTE_MICRO_MOUNT' && data.kind !== 'DANTE_MICRO_UPDATE') ||
          !Number.isSafeInteger(data.requestId) || data.requestId <= 0 || data.props?.contextEpoch !== data.contextEpoch) return
      var snapshot = hydrate(data.props)
      if (!snapshot) {
        send('DANTE_MICRO_ERROR', data.contextEpoch, { requestId: data.requestId, code: 'INVALID_CONTEXT' })
        return
      }
      var taskSequence = ++sequence
      chain = chain.catch(function () {}).then(async function () {
        if (!usable(snapshot) || taskSequence !== sequence) return
        try {
          if (!mounted) { mounted = true; await lifecycle.mount(container, snapshot) }
          else if (typeof lifecycle.update === 'function') await lifecycle.update(snapshot)
          else {
            mounted = false
            await lifecycle.unmount()
            if (!usable(snapshot) || taskSequence !== sequence) return
            mounted = true
            await lifecycle.mount(container, snapshot)
          }
          if (usable(snapshot) && taskSequence === sequence) send('DANTE_MICRO_ACK', snapshot.contextEpoch, { requestId: data.requestId })
        } catch (_) {
          // 任意应用错误文本可能含 token；只向宿主返回可诊断的固定错误码。
          if (usable(snapshot) && taskSequence === sequence) send('DANTE_MICRO_ERROR', snapshot.contextEpoch,
            { requestId: data.requestId, code: 'LIFECYCLE_FAILED' })
        }
      })
    }
    function receiveConnect(event) {
      var data = event.data
      // 不信任 payload 自称 hostOrigin；origin 是浏览器提供且由应用部署配置允许。
      if (stopped || port || event.source !== global.parent || origins.indexOf(event.origin) < 0 ||
          !data || data.kind !== 'DANTE_MICRO_CONNECT' || data.appCode !== options.appCode || data.contractVersion !== VERSION ||
          typeof data.nonce !== 'string' || !/^[a-zA-Z0-9-]{16,128}$/.test(data.nonce) ||
          !Number.isSafeInteger(data.contextEpoch) || data.contextEpoch <= 0 || event.ports.length !== 1 ||
          typeof event.ports[0].postMessage !== 'function') return
      nonce = data.nonce
      port = event.ports[0]
      global.removeEventListener('message', receiveConnect)
      port.onmessage = receive
      port.onmessageerror = dispose
      port.start()
      send('DANTE_MICRO_READY', data.contextEpoch, { appCode: options.appCode, contractVersion: VERSION })
    }
    global.addEventListener('message', receiveConnect)
    global.addEventListener('pagehide', dispose, { once: true })
    return {
      dispose: dispose,
      current: function () { return usable(context) ? context : null },
      /** 只有当前资源的 export 功能权限与数据 grant 都允许时，宿主才发起 CSV 下载。 */
      downloadCsv: function (file) {
        var snapshot = context
        if (!usable(snapshot)) return Promise.reject(abortError())
        if (!file || typeof file.resource !== 'string' || typeof file.filename !== 'string' || typeof file.content !== 'string' ||
            file.content.length > 4 * 1024 * 1024 || new TextEncoder().encode(file.content).byteLength > 4 * 1024 * 1024) {
          return Promise.reject(new Error('Invalid CSV download'))
        }
        if (pendingDownloads.size >= 4) return Promise.reject(new Error('Too many pending CSV downloads'))
        var id = ++nextDownload
        return new Promise(function (resolve, reject) {
          var abort = function () { settleDownload(id, abortError()) }
          var timer = setTimeout(function () { settleDownload(id, new Error('CSV download timed out')) }, 15000)
          pendingDownloads.set(id, { snapshot: snapshot, resolve: resolve, reject: reject, abort: abort, timer: timer })
          snapshot.signal.addEventListener('abort', abort, { once: true })
          if (!usable(snapshot)) { abort(); return }
          send('DANTE_MICRO_DOWNLOAD', snapshot.contextEpoch, { requestId: id,
            resource: file.resource, filename: file.filename, content: file.content })
        })
      },
      /** 应用业务 fetch 必须用此 helper 或直接绑定 props.signal，并校验响应仍属当前 context。 */
      requestJson: async function (url, requestOptions) {
        var snapshot = context
        if (!usable(snapshot)) throw abortError()
        requestOptions = requestOptions || {}
        var request = new AbortController()
        var abort = function () { request.abort() }
        snapshot.signal.addEventListener('abort', abort, { once: true })
        if (requestOptions.signal) requestOptions.signal.addEventListener('abort', abort, { once: true })
        if (snapshot.signal.aborted || requestOptions.signal?.aborted) abort()
        var check = function () { if (!usable(snapshot) || request.signal.aborted) throw abortError() }
        try {
          var headers = new Headers(requestOptions.headers)
          headers.set('Authorization', 'Bearer ' + snapshot.token)
          // Opaque origin deployments require API CORS support; platform cookies are never sent by this helper.
          var response = await global.fetch(url, Object.assign({}, requestOptions, { headers: headers, signal: request.signal, credentials: 'omit' }))
          check()
          var body = await response.json()
          check()
          if (!response.ok) throw new Error('Application request failed: ' + response.status)
          return body
        } finally {
          snapshot.signal.removeEventListener('abort', abort)
          if (requestOptions.signal) requestOptions.signal.removeEventListener('abort', abort)
        }
      },
    }
  }
  global.DanteMicroIframe = Object.freeze({ connect: connect, contractVersion: VERSION })
})(window)
