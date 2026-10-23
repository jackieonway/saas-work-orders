/* Generated SaaS Micro SDK 2.0.0; source: examples/micro-app-sdk/release.json */

// canonical-sdk:sdk-protocol
var SDK_VERSION = "2.0.0";
var CONTEXT_CONTRACT_VERSION = "1.0";
var MICRO_PROPS_CONTRACT_VERSION = "2.0";
var NOTIFICATION_CONTRACT_VERSION = "1.0";
var DEFAULT_MAX_JSON_RESPONSE_BYTES = 1048576;
var MAX_HTTP_RESPONSE_BYTES = 4194304;
var MAX_ORGANIZATION_SCOPE_IDS = 1e4;
var SUPPORTED_SDK_VERSIONS = Object.freeze(["2.0.0"]);
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
export {
  CONTEXT_CONTRACT_VERSION,
  DEFAULT_MAX_JSON_RESPONSE_BYTES,
  DEFAULT_MAX_RESPONSE_BYTES,
  MAX_HTTP_RESPONSE_BYTES,
  MAX_ORGANIZATION_SCOPE_IDS,
  MAX_RESPONSE_BYTES,
  MICRO_PROPS_CONTRACT_VERSION,
  MicroSdkError,
  NOTIFICATION_CONTRACT_VERSION,
  SDK_VERSION,
  SUPPORTED_SDK_VERSIONS,
  assertBrowserCapabilities,
  assertSupportedSdkVersion,
  authorizationContextSignature,
  createContextClient,
  createNotificationPublisher,
  detectBrowserCapabilities,
  isSupportedSdkVersion,
  requestWithContext,
  sameAuthorizationContext
};
