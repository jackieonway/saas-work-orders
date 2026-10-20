/* Generated SaaS Micro SDK 1.0.0; source: examples/micro-app-sdk/release.json */
(() => {
  var __defProp = Object.defineProperty;
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };

  // examples/micro-app-sdk/src/index.mjs
  var index_exports = {};
  __export(index_exports, {
    CONTEXT_CONTRACT_VERSION: () => CONTEXT_CONTRACT_VERSION,
    DEFAULT_MAX_JSON_RESPONSE_BYTES: () => DEFAULT_MAX_JSON_RESPONSE_BYTES,
    DEFAULT_MAX_RESPONSE_BYTES: () => DEFAULT_MAX_RESPONSE_BYTES,
    IFRAME_PROTOCOL_VERSION: () => IFRAME_PROTOCOL_VERSION,
    MAX_HTTP_RESPONSE_BYTES: () => MAX_HTTP_RESPONSE_BYTES,
    MAX_ORGANIZATION_SCOPE_IDS: () => MAX_ORGANIZATION_SCOPE_IDS,
    MAX_RESPONSE_BYTES: () => MAX_RESPONSE_BYTES,
    MICRO_PROPS_CONTRACT_VERSION: () => MICRO_PROPS_CONTRACT_VERSION,
    MicroSdkError: () => MicroSdkError,
    NOTIFICATION_CONTRACT_VERSION: () => NOTIFICATION_CONTRACT_VERSION,
    SDK_VERSION: () => SDK_VERSION,
    SUPPORTED_SDK_VERSIONS: () => SUPPORTED_SDK_VERSIONS,
    assertBrowserCapabilities: () => assertBrowserCapabilities,
    assertSupportedSdkVersion: () => assertSupportedSdkVersion,
    authorizationContextSignature: () => authorizationContextSignature,
    createContextClient: () => createContextClient,
    createNotificationPublisher: () => createNotificationPublisher,
    detectBrowserCapabilities: () => detectBrowserCapabilities,
    installIframeBridge: () => installIframeBridge,
    isSupportedSdkVersion: () => isSupportedSdkVersion,
    requestWithContext: () => requestWithContext,
    sameAuthorizationContext: () => sameAuthorizationContext
  });

  // canonical-sdk:sdk-protocol
  var SDK_VERSION = "1.0.0";
  var CONTEXT_CONTRACT_VERSION = "1.0";
  var MICRO_PROPS_CONTRACT_VERSION = "2.0";
  var IFRAME_PROTOCOL_VERSION = "2.0";
  var NOTIFICATION_CONTRACT_VERSION = "1.0";
  var DEFAULT_MAX_JSON_RESPONSE_BYTES = 1048576;
  var MAX_HTTP_RESPONSE_BYTES = 4194304;
  var MAX_ORGANIZATION_SCOPE_IDS = 1e4;
  var SUPPORTED_SDK_VERSIONS = Object.freeze(["1.0.0"]);
  function isSupportedSdkVersion(value) {
    return typeof value === "string" && SUPPORTED_SDK_VERSIONS.includes(value);
  }
  function assertSupportedSdkVersion(value) {
    if (!isSupportedSdkVersion(value)) {
      const error = new Error("Unsupported SDK version");
      error.code = "UNSUPPORTED_SDK_VERSION";
      throw error;
    }
  }

  // examples/micro-app-sdk/src/http.mjs
  var DEFAULT_MAX_RESPONSE_BYTES = DEFAULT_MAX_JSON_RESPONSE_BYTES;
  var MAX_RESPONSE_BYTES = MAX_HTTP_RESPONSE_BYTES;
  var messages = Object.freeze({
    UNAUTHORIZED: "Application authentication is no longer valid",
    FORBIDDEN: "Application operation is forbidden",
    CONFLICT: "Application operation conflicts with current state",
    NETWORK: "Application service is unavailable",
    PROTOCOL: "Application response does not match the supported protocol",
    HTTP: "Application request failed"
  });
  var uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
  var MicroSdkError = class extends Error {
    constructor(kind, { status = 0, requestId, code } = {}) {
      super(messages[kind] || messages.PROTOCOL);
      this.name = "MicroSdkError";
      this.kind = Object.hasOwn(messages, kind) ? kind : "PROTOCOL";
      this.status = Number.isInteger(status) && status >= 0 && status <= 599 ? status : 0;
      this.code = typeof code === "string" && /^[A-Z][A-Z0-9_]{0,63}$/.test(code) ? code : this.kind;
      if (typeof requestId === "string" && uuid.test(requestId)) this.requestId = requestId;
    }
    toJSON() {
      return {
        kind: this.kind,
        status: this.status,
        code: this.code,
        message: this.message,
        ...this.requestId === void 0 ? {} : { requestId: this.requestId }
      };
    }
  };
  var contextAbortError = () => new DOMException("Application context cancelled or expired", "AbortError");
  function protocolError(code = "INVALID_PROTOCOL") {
    return new MicroSdkError("PROTOCOL", { code });
  }
  function statusKind(status) {
    return status === 401 ? "UNAUTHORIZED" : status === 403 ? "FORBIDDEN" : status === 409 ? "CONFLICT" : "HTTP";
  }
  function abortable(promise, signal) {
    return new Promise((resolve, reject) => {
      const abort = () => {
        cleanup();
        reject(contextAbortError());
      };
      const cleanup = () => signal.removeEventListener("abort", abort);
      signal.addEventListener("abort", abort, { once: true });
      Promise.resolve(promise).then((value) => {
        cleanup();
        resolve(value);
      }, (error) => {
        cleanup();
        reject(error);
      });
      if (signal.aborted) abort();
    });
  }
  async function readLimited(response, check, maximum, preserveBom, requestId, signal) {
    const fail = () => new MicroSdkError("PROTOCOL", { status: 413, requestId, code: "RESPONSE_TOO_LARGE" });
    const declared = response.headers?.get("Content-Length");
    if (declared !== null && declared !== void 0 && /^\d+$/.test(declared) && Number(declared) > maximum) {
      response.body?.cancel?.().catch(() => {
      });
      check();
      throw fail();
    }
    if (response.body === null) return "";
    if (typeof response.body?.getReader !== "function") throw new MicroSdkError("PROTOCOL", { requestId, code: "INVALID_RESPONSE" });
    const reader = response.body.getReader();
    const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: preserveBom });
    const parts = [];
    let bytes = 0, completed = false;
    try {
      for (; ; ) {
        const next = await abortable(reader.read(), signal);
        check();
        if (next.done) {
          completed = true;
          break;
        }
        if (!(next.value instanceof Uint8Array)) throw new MicroSdkError("PROTOCOL", { requestId, code: "INVALID_RESPONSE" });
        bytes += next.value.byteLength;
        if (bytes > maximum) {
          reader.cancel().catch(() => {
          });
          check();
          throw fail();
        }
        try {
          parts.push(decoder.decode(next.value, { stream: true }));
        } catch {
          throw new MicroSdkError("PROTOCOL", { requestId, code: "INVALID_UTF8" });
        }
      }
      try {
        parts.push(decoder.decode());
      } catch {
        throw new MicroSdkError("PROTOCOL", { requestId, code: "INVALID_UTF8" });
      }
      return parts.join("");
    } finally {
      if (!completed) reader.cancel().catch(() => {
      });
      reader.releaseLock();
    }
  }
  async function requestWithContext({
    props,
    url,
    options = {},
    fetchImpl = globalThis.fetch,
    check = () => {
    },
    responseType = "json",
    maxBytes = DEFAULT_MAX_RESPONSE_BYTES
  }) {
    if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0 || maxBytes > MAX_RESPONSE_BYTES || !["json", "text"].includes(responseType)) throw protocolError("INVALID_RESPONSE_LIMIT");
    try {
      if (props?.sdkVersion !== void 0) assertSupportedSdkVersion(props.sdkVersion);
    } catch {
      throw protocolError("UNSUPPORTED_SDK_VERSION");
    }
    if (props?.contractVersion !== MICRO_PROPS_CONTRACT_VERSION || typeof props.token !== "string" || !props.token || !props.signal || typeof props.signal.addEventListener !== "function") throw protocolError("UNSUPPORTED_MICRO_PROPS");
    const request = new AbortController();
    const abort = () => request.abort();
    const signals = [...new Set([props.signal, options.signal].filter(Boolean))];
    signals.forEach((signal) => signal.addEventListener("abort", abort, { once: true }));
    if (signals.some((signal) => signal.aborted)) abort();
    const verify = () => {
      check();
      if (request.signal.aborted || !(Date.parse(props.expiresAt) > Date.now()) || !(Date.parse(props.tokenExpiresAt) > Date.now())) throw contextAbortError();
    };
    let requestId;
    try {
      verify();
      const headers = new Headers(options.headers);
      const supplied = headers.get("X-Request-Id");
      requestId = supplied && uuid.test(supplied) ? supplied : globalThis.crypto.randomUUID();
      headers.set("X-Request-Id", requestId);
      headers.set("Authorization", "Bearer " + props.token);
      let response;
      try {
        const fetchRequest = fetchImpl;
        response = await abortable(fetchRequest(url, { ...options, headers, signal: request.signal, credentials: "omit" }), request.signal);
      } catch (error) {
        verify();
        if (error?.name === "AbortError") throw contextAbortError();
        throw new MicroSdkError("NETWORK", { requestId });
      }
      verify();
      if (!response || !Number.isInteger(response.status) || response.status < 100 || response.status > 599 || typeof response.ok !== "boolean" || !response.headers || typeof response.headers.get !== "function") {
        throw new MicroSdkError("PROTOCOL", { requestId, code: "INVALID_RESPONSE" });
      }
      const echoed = response.headers.get("X-Request-Id");
      if (echoed && uuid.test(echoed)) requestId = echoed;
      if (!response.ok) {
        response.body?.cancel?.().catch(() => {
        });
        verify();
        throw new MicroSdkError(statusKind(response.status), { status: response.status, requestId });
      }
      let text;
      try {
        text = await readLimited(response, verify, maxBytes, responseType === "text", requestId, request.signal);
      } catch (error) {
        verify();
        if (error instanceof MicroSdkError) throw error;
        if (error?.name === "AbortError") throw contextAbortError();
        throw new MicroSdkError("NETWORK", { requestId });
      }
      verify();
      if (responseType === "text") return text;
      try {
        return JSON.parse(text);
      } catch {
        throw new MicroSdkError("PROTOCOL", { status: response.status, requestId, code: "INVALID_JSON" });
      }
    } catch (error) {
      if (error instanceof MicroSdkError) throw error;
      if (request.signal.aborted || error?.name === "AbortError") throw contextAbortError();
      throw new MicroSdkError("PROTOCOL", { requestId, code: "INVALID_REQUEST" });
    } finally {
      signals.forEach((signal) => signal.removeEventListener("abort", abort));
    }
  }

  // examples/micro-app-sdk/src/capabilities.mjs
  var clientChecks = Object.freeze([
    ["fetch", (env) => typeof env.fetch === "function"],
    ["Headers", (env) => typeof env.Headers === "function"],
    ["URL", (env) => typeof env.URL === "function"],
    ["AbortController", (env) => typeof env.AbortController === "function"],
    ["DOMException", (env) => typeof env.DOMException === "function"],
    ["TextEncoder", (env) => typeof env.TextEncoder === "function"],
    ["TextDecoder", (env) => typeof env.TextDecoder === "function"],
    ["ReadableStream", (env) => typeof env.ReadableStream === "function"],
    ["Response.body", (env) => typeof env.Response === "function" && "body" in env.Response.prototype],
    ["crypto.randomUUID", (env) => typeof env.crypto?.randomUUID === "function"]
  ]);
  var modeChecks = Object.freeze({
    client: clientChecks,
    iframe: Object.freeze([
      ...clientChecks,
      ["MessageChannel", (env) => typeof env.MessageChannel === "function"],
      ["postMessage", (env) => typeof env.postMessage === "function"],
      ["addEventListener", (env) => typeof env.addEventListener === "function"],
      ["removeEventListener", (env) => typeof env.removeEventListener === "function"]
    ]),
    host: Object.freeze([
      ...clientChecks,
      ["AbortSignal.any", (env) => typeof env.AbortSignal?.any === "function"],
      ["AbortSignal.timeout", (env) => typeof env.AbortSignal?.timeout === "function"],
      ["MessageChannel", (env) => typeof env.MessageChannel === "function"],
      ["MutationObserver", (env) => typeof env.MutationObserver === "function"],
      ["iframe.sandbox", (env) => typeof env.HTMLIFrameElement === "function" && "sandbox" in env.HTMLIFrameElement.prototype],
      ["document.createElement", (env) => typeof env.document?.createElement === "function"],
      ["Blob", (env) => typeof env.Blob === "function"],
      ["URL.createObjectURL", (env) => typeof env.URL?.createObjectURL === "function"],
      ["URL.revokeObjectURL", (env) => typeof env.URL?.revokeObjectURL === "function"]
    ])
  });
  function detectBrowserCapabilities(mode = "client", environment = globalThis) {
    if (!Object.hasOwn(modeChecks, mode)) throw new TypeError("Unsupported SDK capability mode");
    const missing = modeChecks[mode].filter(([, check]) => {
      try {
        return !check(environment);
      } catch {
        return true;
      }
    }).map(([name]) => name);
    return Object.freeze({ mode, supported: missing.length === 0, missing: Object.freeze(missing) });
  }
  function assertBrowserCapabilities(mode = "client", environment = globalThis) {
    const result = detectBrowserCapabilities(mode, environment);
    if (!result.supported) {
      const error = new Error("Required browser capabilities are unavailable: " + result.missing.join(", "));
      error.code = "BROWSER_CAPABILITIES_UNSUPPORTED";
      error.missing = result.missing;
      throw error;
    }
    return result;
  }

  // examples/micro-app-sdk/src/context-updates.mjs
  var authorizationFields = Object.freeze([
    "sdkVersion",
    "contractVersion",
    "appCode",
    "basePath",
    "contextEpoch",
    "contextVersion",
    "token",
    "expiresAt",
    "tokenExpiresAt",
    "deploymentVersion",
    "entryVersionId",
    "user",
    "tenant",
    "organization",
    "organizationScopeIds",
    "roles",
    "permissions",
    "dataGrants",
    "menus"
  ]);
  var knownFields = /* @__PURE__ */ new Set([...authorizationFields, "signal", "theme", "routePath", "onNavigate", "onNotify"]);
  function canonical(value) {
    if (value === void 0) return ["undefined"];
    if (value === null) return ["null"];
    if (typeof value === "string" || typeof value === "boolean") return [typeof value, value];
    if (typeof value === "number" && Number.isFinite(value)) return ["number", value];
    if (Array.isArray(value)) return ["array", value.map(canonical)];
    if (Object.prototype.toString.call(value) === "[object Object]") {
      return ["object", Object.keys(value).sort().map((key) => [key, canonical(value[key])])];
    }
    throw new TypeError("Unsupported context value");
  }
  function authorizationContextSignature(props) {
    if (!props || typeof props !== "object" || Object.keys(props).some((key) => !knownFields.has(key))) return null;
    try {
      return JSON.stringify(authorizationFields.map((key) => [
        key,
        Object.hasOwn(props, key) ? canonical(props[key]) : ["missing"]
      ]));
    } catch {
      return null;
    }
  }
  function sameAuthorizationContext(previous, next) {
    const before = authorizationContextSignature(previous), after = authorizationContextSignature(next);
    return before !== null && after !== null && before === after;
  }

  // examples/micro-app-sdk/src/context-client.mjs
  function createContextClient(initialProps, fetchImpl = globalThis.fetch) {
    assertBrowserCapabilities("client");
    let props, controller, unbind, hostSignal, authorizationKey, generation = 0;
    const usable = () => props && !controller.signal.aborted && Date.parse(props.expiresAt) > Date.now() && Date.parse(props.tokenExpiresAt) > Date.now();
    const stop = () => {
      generation++;
      controller?.abort();
      unbind?.();
      unbind = null;
    };
    const update = (next) => {
      try {
        if (next?.sdkVersion !== void 0) assertSupportedSdkVersion(next.sdkVersion);
      } catch {
        stop();
        throw protocolError("UNSUPPORTED_SDK_VERSION");
      }
      if (next?.contractVersion !== MICRO_PROPS_CONTRACT_VERSION || typeof next.token !== "string" || !next.token || !next.signal || typeof next.signal.addEventListener !== "function" || !Number.isSafeInteger(next.contextEpoch) || next.contextEpoch <= 0 || !next.contextVersion || !next.user?.id || !next.tenant?.id || !next.organization?.id) {
        stop();
        throw protocolError("UNSUPPORTED_MICRO_PROPS");
      }
      const nextKey = authorizationContextSignature(next);
      if (!usable() || nextKey === null || nextKey !== authorizationKey || hostSignal !== next.signal) {
        stop();
        const scope = new AbortController();
        controller = scope;
        const abort = () => scope.abort();
        hostSignal = next.signal;
        const source = hostSignal;
        source.addEventListener("abort", abort, { once: true });
        unbind = () => source.removeEventListener("abort", abort);
        if (source.aborted) abort();
      }
      authorizationKey = nextKey;
      props = next;
    };
    update(initialProps);
    return {
      sdkVersion: SDK_VERSION,
      update,
      dispose: stop,
      current: () => usable() ? props : null,
      async requestJson(url, options = {}) {
        if (!usable()) throw contextAbortError();
        const requestGeneration = generation, snapshot = props, lifetime = controller.signal;
        const check = () => {
          if (requestGeneration !== generation || lifetime.aborted || !usable()) throw contextAbortError();
        };
        const scope = new AbortController(), abort = () => scope.abort();
        lifetime.addEventListener("abort", abort, { once: true });
        options.signal?.addEventListener("abort", abort, { once: true });
        if (lifetime.aborted || options.signal?.aborted) abort();
        try {
          return await requestWithContext({ props: snapshot, url, options: { ...options, signal: scope.signal }, fetchImpl, check });
        } finally {
          lifetime.removeEventListener("abort", abort);
          options.signal?.removeEventListener("abort", abort);
        }
      }
    };
  }

  // examples/micro-app-sdk/src/notification-publisher.mjs
  function createNotificationPublisher(client) {
    if (!client || typeof client.current !== "function" || typeof client.requestJson !== "function") {
      throw new TypeError("An active micro application context client is required");
    }
    try {
      if (client.sdkVersion !== void 0) assertSupportedSdkVersion(client.sdkVersion);
    } catch {
      throw protocolError("UNSUPPORTED_SDK_VERSION");
    }
    return {
      sdkVersion: SDK_VERSION,
      async publish(body) {
        const current = client.current();
        if (!current?.appCode) throw contextAbortError();
        if (typeof body?.idempotencyKey !== "string" || !body.idempotencyKey.trim()) {
          throw protocolError("INVALID_NOTIFICATION");
        }
        let payload;
        try {
          payload = JSON.stringify(body);
        } catch {
          throw protocolError("INVALID_NOTIFICATION");
        }
        if (new TextEncoder().encode(payload).byteLength > 64 * 1024) throw protocolError("NOTIFICATION_TOO_LARGE");
        return client.requestJson(`/api/apps/${encodeURIComponent(current.appCode)}/notifications`, {
          method: "POST",
          credentials: "omit",
          headers: { "Content-Type": "application/json" },
          body: payload
        });
      }
    };
  }

  // examples/micro-app-sdk/src/iframe-bridge.js
  function installIframeBridge(global) {
    "use strict";
    var VERSION = IFRAME_PROTOCOL_VERSION;
    var variants = ["default", "success", "error", "warning", "info"];
    function abortError() {
      return new DOMException("Application context cancelled or expired", "AbortError");
    }
    function connect(options) {
      assertBrowserCapabilities("iframe", global);
      try {
        if (options?.sdkVersion !== void 0) assertSupportedSdkVersion(options.sdkVersion);
      } catch {
        throw protocolError("UNSUPPORTED_SDK_VERSION");
      }
      if (!options || typeof options.appCode !== "string" || !options.appCode || !Array.isArray(options.allowedHostOrigins) || options.allowedHostOrigins.length === 0 || !options.lifecycle || typeof options.lifecycle.mount !== "function" || typeof options.lifecycle.unmount !== "function") {
        throw new Error("MicroIframe requires appCode, explicit allowedHostOrigins and mount/unmount lifecycle");
      }
      var origins = options.allowedHostOrigins.map(function(origin) {
        var url = new URL(origin);
        if (!/^https?:$/.test(url.protocol) || origin !== url.origin) throw new Error("allowedHostOrigins must contain exact HTTP(S) origins");
        return origin;
      });
      var lifecycle = options.lifecycle;
      var container = options.container || document.body;
      var port = null, nonce = null, context = null, authorizationKey = null, sequence = 0, mounted = false, stopped = false;
      var expiryTimer = null, chain = Promise.resolve();
      var nextDownload = 0, pendingDownloads = /* @__PURE__ */ new Map();
      function settleDownload(id, error) {
        var pending = pendingDownloads.get(id);
        if (!pending) return;
        pendingDownloads.delete(id);
        clearTimeout(pending.timer);
        pending.snapshot.signal.removeEventListener("abort", pending.abort);
        if (error) pending.reject(error);
        else pending.resolve();
      }
      function usable(snapshot) {
        return !stopped && context && snapshot && context.controller === snapshot.controller && !snapshot.signal.aborted && Date.parse(snapshot.expiresAt) > Date.now() && Date.parse(snapshot.tokenExpiresAt) > Date.now();
      }
      function send(kind, epoch, extra) {
        if (port && !stopped) port.postMessage(Object.assign({ kind, nonce, contextEpoch: epoch }, extra));
      }
      function abortContext() {
        if (context) context.controller.abort();
        pendingDownloads.forEach(function(_, id) {
          settleDownload(id, abortError());
        });
        if (expiryTimer !== null) clearTimeout(expiryTimer);
        expiryTimer = null;
      }
      function dispose() {
        if (stopped) return;
        stopped = true;
        sequence++;
        abortContext();
        context = null;
        global.removeEventListener("message", receiveConnect);
        global.removeEventListener("pagehide", dispose);
        if (port) {
          port.onmessage = null;
          port.onmessageerror = null;
          port.close();
          port = null;
        }
        chain = chain.catch(function() {
        }).then(function() {
          if (mounted) {
            mounted = false;
            return lifecycle.unmount();
          }
        }).catch(function() {
        });
      }
      function hydrate(dto) {
        if (context && Number.isSafeInteger(dto?.contextEpoch) && dto.contextEpoch < context.contextEpoch) return null;
        try {
          if (dto?.sdkVersion !== void 0) assertSupportedSdkVersion(dto.sdkVersion);
        } catch {
          abortContext();
          return null;
        }
        var expiry = dto && Math.min(Date.parse(dto.expiresAt), Date.parse(dto.tokenExpiresAt));
        if (!dto || dto.contractVersion !== VERSION || dto.appCode !== options.appCode || dto.basePath !== "/micro/" + options.appCode || !Number.isSafeInteger(dto.contextEpoch) || dto.contextEpoch <= 0 || !dto.contextVersion || typeof dto.token !== "string" || !dto.token || !Number.isFinite(expiry) || expiry <= Date.now() || !dto.user || !dto.user.id || !dto.tenant || !dto.tenant.id || !dto.organization || !dto.organization.id || context && dto.contextEpoch < context.contextEpoch) {
          abortContext();
          return null;
        }
        var nextKey = authorizationContextSignature(dto);
        var reuse = usable(context) && nextKey !== null && nextKey === authorizationKey && (typeof lifecycle.update === "function" || context.routePath === dto.routePath);
        if (!reuse) abortContext();
        var controller = reuse ? context.controller : new AbortController();
        var snapshot = Object.assign({}, dto, { signal: controller.signal });
        Object.defineProperty(snapshot, "controller", { value: controller, enumerable: false });
        snapshot.onNavigate = function(path) {
          if (usable(snapshot) && typeof path === "string") send("MICRO_NAVIGATE", snapshot.contextEpoch, { path });
        };
        snapshot.onNotify = function(message, variant) {
          variant = variant === void 0 ? "default" : variant;
          if (usable(snapshot) && typeof message === "string" && message.length > 0 && message.length <= 1e3 && variants.indexOf(variant) >= 0) {
            send("MICRO_NOTIFY", snapshot.contextEpoch, { message, variant });
          }
        };
        context = snapshot;
        authorizationKey = nextKey;
        if (!reuse) expiryTimer = setTimeout(function() {
          if (context && context.controller === controller) abortContext();
        }, Math.max(1, expiry - Date.now()));
        return snapshot;
      }
      function receive(event) {
        var data = event.data;
        if (stopped || !data || typeof data !== "object" || data.nonce !== nonce || !Number.isSafeInteger(data.contextEpoch)) return;
        if (data.kind === "MICRO_DOWNLOAD_RESULT") {
          var pending = pendingDownloads.get(data.requestId);
          if (pending && usable(pending.snapshot) && data.contextEpoch === pending.snapshot.contextEpoch) {
            settleDownload(data.requestId, data.ok === true ? void 0 : new Error("CSV download denied"));
          }
          return;
        }
        if (data.kind === "MICRO_UNMOUNT") {
          if (!context || data.contextEpoch === context.contextEpoch) dispose();
          return;
        }
        if (data.kind === "MICRO_CONTEXT_ABORT") {
          if (context && data.contextEpoch === context.contextEpoch) {
            sequence++;
            abortContext();
          }
          return;
        }
        if (data.kind !== "MICRO_MOUNT" && data.kind !== "MICRO_UPDATE" || !Number.isSafeInteger(data.requestId) || data.requestId <= 0 || data.props?.contextEpoch !== data.contextEpoch) return;
        var previous = context;
        var snapshot = hydrate(data.props);
        if (!snapshot) {
          send("MICRO_ERROR", data.contextEpoch, { requestId: data.requestId, code: "INVALID_CONTEXT" });
          return;
        }
        var taskSequence = ++sequence;
        chain = chain.catch(function() {
        }).then(async function() {
          if (!usable(snapshot) || taskSequence !== sequence) return;
          try {
            if (!mounted) {
              mounted = true;
              await lifecycle.mount(container, snapshot);
            } else if (typeof lifecycle.update === "function") await lifecycle.update(snapshot);
            else if (!previous || previous.controller !== snapshot.controller || previous.routePath !== snapshot.routePath) {
              mounted = false;
              await lifecycle.unmount();
              if (!usable(snapshot) || taskSequence !== sequence) return;
              mounted = true;
              await lifecycle.mount(container, snapshot);
            }
            if (usable(snapshot) && taskSequence === sequence) send("MICRO_ACK", snapshot.contextEpoch, { requestId: data.requestId });
          } catch (_) {
            if (usable(snapshot) && taskSequence === sequence) send(
              "MICRO_ERROR",
              snapshot.contextEpoch,
              { requestId: data.requestId, code: "LIFECYCLE_FAILED" }
            );
          }
        });
      }
      function receiveConnect(event) {
        var data = event.data;
        if (stopped || port || event.source !== global.parent || origins.indexOf(event.origin) < 0 || !data || data.kind !== "MICRO_CONNECT" || data.appCode !== options.appCode || data.contractVersion !== VERSION || typeof data.nonce !== "string" || !/^[a-zA-Z0-9-]{16,128}$/.test(data.nonce) || !Number.isSafeInteger(data.contextEpoch) || data.contextEpoch <= 0 || event.ports.length !== 1 || typeof event.ports[0].postMessage !== "function") return;
        if (data.sdkVersion !== void 0) {
          try {
            assertSupportedSdkVersion(data.sdkVersion);
          } catch {
            event.ports[0].postMessage({ kind: "MICRO_ERROR", nonce: data.nonce, contextEpoch: data.contextEpoch, code: "UNSUPPORTED_SDK_VERSION" });
            event.ports[0].close();
            return;
          }
        }
        nonce = data.nonce;
        port = event.ports[0];
        global.removeEventListener("message", receiveConnect);
        port.onmessage = receive;
        port.onmessageerror = dispose;
        port.start();
        send("MICRO_READY", data.contextEpoch, { appCode: options.appCode, contractVersion: VERSION, sdkVersion: SDK_VERSION });
      }
      global.addEventListener("message", receiveConnect);
      global.addEventListener("pagehide", dispose, { once: true });
      return {
        sdkVersion: SDK_VERSION,
        dispose,
        current: function() {
          return usable(context) ? context : null;
        },
        /** 只有当前资源的 export 功能权限与数据 grant 都允许时，宿主才发起 CSV 下载。 */
        downloadCsv: function(file) {
          var snapshot = context;
          if (!usable(snapshot)) return Promise.reject(abortError());
          if (!file || typeof file.resource !== "string" || typeof file.filename !== "string" || typeof file.content !== "string" || file.content.length > 4 * 1024 * 1024 || new TextEncoder().encode(file.content).byteLength > 4 * 1024 * 1024) {
            return Promise.reject(new Error("Invalid CSV download"));
          }
          if (pendingDownloads.size >= 4) return Promise.reject(new Error("Too many pending CSV downloads"));
          var id = ++nextDownload;
          return new Promise(function(resolve, reject) {
            var abort = function() {
              settleDownload(id, abortError());
            };
            var timer = setTimeout(function() {
              settleDownload(id, new Error("CSV download timed out"));
            }, 15e3);
            pendingDownloads.set(id, { snapshot, resolve, reject, abort, timer });
            snapshot.signal.addEventListener("abort", abort, { once: true });
            if (!usable(snapshot)) {
              abort();
              return;
            }
            send("MICRO_DOWNLOAD", snapshot.contextEpoch, {
              requestId: id,
              resource: file.resource,
              filename: file.filename,
              content: file.content
            });
          });
        },
        /** 应用业务 fetch 必须用此 helper 或直接绑定 props.signal，并校验响应仍属当前 context。 */
        requestJson: async function(url, requestOptions) {
          var snapshot = context;
          if (!usable(snapshot)) throw abortError();
          requestOptions = requestOptions || {};
          var request = new AbortController();
          var abort = function() {
            request.abort();
          };
          snapshot.signal.addEventListener("abort", abort, { once: true });
          if (requestOptions.signal) requestOptions.signal.addEventListener("abort", abort, { once: true });
          if (snapshot.signal.aborted || requestOptions.signal?.aborted) abort();
          var check = function() {
            if (!usable(snapshot) || request.signal.aborted) throw abortError();
          };
          try {
            return await requestWithContext({
              props: snapshot,
              url,
              options: Object.assign({}, requestOptions, { signal: request.signal }),
              fetchImpl: global.fetch,
              check
            });
          } finally {
            snapshot.signal.removeEventListener("abort", abort);
            if (requestOptions.signal) requestOptions.signal.removeEventListener("abort", abort);
          }
        }
      };
    }
    global.MicroIframe = Object.freeze({ connect, contractVersion: VERSION, sdkVersion: SDK_VERSION });
  }

  // examples/micro-app-sdk/src/classic-entry.mjs
  window.MicroSdk = Object.freeze({ ...index_exports, sdkVersion: SDK_VERSION });
  installIframeBridge(window);
})();
