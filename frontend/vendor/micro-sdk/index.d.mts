import type { MicroProps, NotificationPublishBody, NotificationPublication } from './types.js'
import { SDK_VERSION } from './protocol.mjs'
export * from './protocol.mjs'
export type * from './types.js'
export { MicroSdkError, DEFAULT_MAX_RESPONSE_BYTES, MAX_RESPONSE_BYTES, requestWithContext } from './http.mjs'
export type { MicroSdkErrorKind, ContextRequestOptions } from './http.mjs'
export { detectBrowserCapabilities, assertBrowserCapabilities } from './capabilities.mjs'
/** Memory-only key contains credentials; unknown context fields refuse reuse. Never log the result. */
export function authorizationContextSignature(props: unknown): string | null
export function sameAuthorizationContext(previous: unknown, next: unknown): boolean

export interface ContextClient {
  readonly sdkVersion: typeof SDK_VERSION
  current(): MicroProps | null
  update(props: MicroProps): void
  dispose(): void
  requestJson<T = unknown>(url: string | URL, options?: RequestInit): Promise<T>
}
export function createContextClient(props: MicroProps, fetchImpl?: typeof fetch): ContextClient
export interface NotificationPublisher {
  readonly sdkVersion: typeof SDK_VERSION
  publish<T = NotificationPublication>(body: NotificationPublishBody): Promise<T>
}
export function createNotificationPublisher(client: Pick<ContextClient, 'current' | 'requestJson'> & { readonly sdkVersion?: string }): NotificationPublisher

declare global {
  interface Window {
    MicroSdk: typeof import('./index.mjs') & { readonly sdkVersion: typeof SDK_VERSION }
  }
}
