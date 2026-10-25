/* Generated SaaS Micro SDK 3.0.0; source: examples/micro-app-sdk/release.json */
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
    isSupportedSdkVersion: () => isSupportedSdkVersion,
    requestWithContext: () => requestWithContext,
    sameAuthorizationContext: () => sameAuthorizationContext
  });

  // canonical-sdk:sdk-protocol
  var SDK_VERSION = "3.0.0";
  var CONTEXT_CONTRACT_VERSION = "1.0";
  var MICRO_PROPS_CONTRACT_VERSION = "2.0";
  var NOTIFICATION_CONTRACT_VERSION = "1.0";
  var DEFAULT_MAX_JSON_RESPONSE_BYTES = 1048576;
  var MAX_HTTP_RESPONSE_BYTES = 4194304;
  var MAX_ORGANIZATION_SCOPE_IDS = 1e4;
  var SUPPORTED_SDK_VERSIONS = Object.freeze(["3.0.0"]);
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

  // D:/WorkSpace/rust/rust-saas/saas-micro-app-sdk/node_modules/uuid/dist/stringify.js
  var byteToHex = [];
  for (let i = 0; i < 256; ++i) {
    byteToHex.push((i + 256).toString(16).slice(1));
  }
  function unsafeStringify(arr, offset = 0) {
    return (byteToHex[arr[offset + 0]] + byteToHex[arr[offset + 1]] + byteToHex[arr[offset + 2]] + byteToHex[arr[offset + 3]] + "-" + byteToHex[arr[offset + 4]] + byteToHex[arr[offset + 5]] + "-" + byteToHex[arr[offset + 6]] + byteToHex[arr[offset + 7]] + "-" + byteToHex[arr[offset + 8]] + byteToHex[arr[offset + 9]] + "-" + byteToHex[arr[offset + 10]] + byteToHex[arr[offset + 11]] + byteToHex[arr[offset + 12]] + byteToHex[arr[offset + 13]] + byteToHex[arr[offset + 14]] + byteToHex[arr[offset + 15]]).toLowerCase();
  }

  // D:/WorkSpace/rust/rust-saas/saas-micro-app-sdk/node_modules/uuid/dist/rng.js
  var getRandomValues;
  var rnds8 = new Uint8Array(16);
  function rng() {
    if (!getRandomValues) {
      if (typeof crypto === "undefined" || !crypto.getRandomValues) {
        throw new Error("crypto.getRandomValues() not supported. See https://github.com/uuidjs/uuid#getrandomvalues-not-supported");
      }
      getRandomValues = crypto.getRandomValues.bind(crypto);
    }
    return getRandomValues(rnds8);
  }

  // D:/WorkSpace/rust/rust-saas/saas-micro-app-sdk/node_modules/uuid/dist/v7.js
  var _state = {};
  function v7(options, buf, offset) {
    let bytes;
    if (options) {
      bytes = v7Bytes(options.random ?? options.rng?.() ?? rng(), options.msecs, options.seq, buf, offset);
    } else {
      const now = Date.now();
      const rnds = rng();
      updateV7State(_state, now, rnds);
      bytes = v7Bytes(rnds, _state.msecs, _state.seq, buf, offset);
    }
    return buf ?? unsafeStringify(bytes);
  }
  function updateV7State(state, now, rnds) {
    state.msecs ??= -Infinity;
    state.seq ??= 0;
    if (now > state.msecs) {
      state.seq = rnds[6] << 23 | rnds[7] << 16 | rnds[8] << 8 | rnds[9];
      state.msecs = now;
    } else {
      state.seq = state.seq + 1 | 0;
      if (state.seq === 0) {
        state.msecs++;
      }
    }
    return state;
  }
  function v7Bytes(rnds, msecs, seq, buf, offset = 0) {
    if (rnds.length < 16) {
      throw new Error("Random bytes length must be >= 16");
    }
    if (!buf) {
      buf = new Uint8Array(16);
      offset = 0;
    } else {
      if (offset < 0 || offset + 16 > buf.length) {
        throw new RangeError(`UUID byte range ${offset}:${offset + 15} is out of buffer bounds`);
      }
    }
    msecs ??= Date.now();
    seq ??= rnds[6] * 127 << 24 | rnds[7] << 16 | rnds[8] << 8 | rnds[9];
    buf[offset++] = msecs / 1099511627776 & 255;
    buf[offset++] = msecs / 4294967296 & 255;
    buf[offset++] = msecs / 16777216 & 255;
    buf[offset++] = msecs / 65536 & 255;
    buf[offset++] = msecs / 256 & 255;
    buf[offset++] = msecs & 255;
    buf[offset++] = 112 | seq >>> 28 & 15;
    buf[offset++] = seq >>> 20 & 255;
    buf[offset++] = 128 | seq >>> 14 & 63;
    buf[offset++] = seq >>> 6 & 255;
    buf[offset++] = seq << 2 & 255 | rnds[10] & 3;
    buf[offset++] = rnds[11];
    buf[offset++] = rnds[12];
    buf[offset++] = rnds[13];
    buf[offset++] = rnds[14];
    buf[offset++] = rnds[15];
    return buf;
  }
  var v7_default = v7;

  // examples/micro-app-sdk/src/uuid.mjs
  var newUuid = () => v7_default();
  var isUuidV7 = (value) => typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value);
  var ids = (value) => Array.isArray(value) && value.every(isUuidV7);
  function menus(value) {
    if (!Array.isArray(value)) return false;
    const pending = [...value], seen = /* @__PURE__ */ new Set();
    while (pending.length) {
      const menu = pending.pop();
      if (!isUuidV7(menu?.id) || menu.parentId != null && !isUuidV7(menu.parentId) || seen.has(menu)) return false;
      seen.add(menu);
      if (menu.children !== void 0) {
        if (!Array.isArray(menu.children)) return false;
        for (const child of menu.children) pending.push(child);
      }
    }
    return true;
  }
  function hasV7ContextIds(props) {
    return isUuidV7(props?.user?.id) && isUuidV7(props?.tenant?.id) && isUuidV7(props?.organization?.id) && (props.organizationScopeIds == null || ids(props.organizationScopeIds)) && (props.roles === void 0 || Array.isArray(props.roles) && props.roles.every((role) => isUuidV7(role?.id))) && (props.menus === void 0 || menus(props.menus)) && (props.dataGrants === void 0 || Array.isArray(props.dataGrants) && props.dataGrants.every((grant) => isUuidV7(grant?.subjectUserId) && isUuidV7(grant?.tenantId) && ids(grant?.organizationIds))) && (props.entryVersionId == null || isUuidV7(props.entryVersionId));
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
  var MicroSdkError = class extends Error {
    constructor(kind, { status = 0, requestId, code } = {}) {
      super(messages[kind] || messages.PROTOCOL);
      this.name = "MicroSdkError";
      this.kind = Object.hasOwn(messages, kind) ? kind : "PROTOCOL";
      this.status = Number.isInteger(status) && status >= 0 && status <= 599 ? status : 0;
      this.code = typeof code === "string" && /^[A-Z][A-Z0-9_]{0,63}$/.test(code) ? code : this.kind;
      if (typeof requestId === "string" && isUuidV7(requestId)) this.requestId = requestId;
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
    if (props?.contractVersion !== MICRO_PROPS_CONTRACT_VERSION || !hasV7ContextIds(props) || typeof props.token !== "string" || !props.token || !props.signal || typeof props.signal.addEventListener !== "function") throw protocolError("UNSUPPORTED_MICRO_PROPS");
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
      requestId = supplied && isUuidV7(supplied) ? supplied : newUuid();
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
      if (echoed && isUuidV7(echoed)) requestId = echoed;
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
    ["crypto.getRandomValues", (env) => typeof env.crypto?.getRandomValues === "function"]
  ]);
  var modeChecks = Object.freeze({
    client: clientChecks
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
      if (next?.contractVersion !== MICRO_PROPS_CONTRACT_VERSION || typeof next.token !== "string" || !next.token || !next.signal || typeof next.signal.addEventListener !== "function" || !Number.isSafeInteger(next.contextEpoch) || next.contextEpoch <= 0 || !next.contextVersion || !hasV7ContextIds(next)) {
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
        const target = body.target;
        if (target?.kind === "USER" && (!isUuidV7(target.userId) || !isUuidV7(target.organizationId)) || target?.kind === "ORGANIZATION" && !isUuidV7(target.organizationId)) {
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

  // examples/micro-app-sdk/src/classic-entry.mjs
  window.MicroSdk = Object.freeze({ ...index_exports, sdkVersion: SDK_VERSION });
})();
