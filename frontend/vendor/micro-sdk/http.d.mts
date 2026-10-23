import type { MicroProps, PublicRequestError } from './types.js'
export type MicroSdkErrorKind = 'UNAUTHORIZED' | 'FORBIDDEN' | 'CONFLICT' | 'NETWORK' | 'PROTOCOL' | 'HTTP'
export class MicroSdkError extends Error {
  constructor(kind: MicroSdkErrorKind, options?: { status?: number; requestId?: string; code?: string })
  readonly kind: MicroSdkErrorKind
  readonly status: number
  readonly code: string
  readonly requestId?: string
  toJSON(): PublicRequestError
}
export const DEFAULT_MAX_RESPONSE_BYTES: number
export const MAX_RESPONSE_BYTES: number
export function contextAbortError(): DOMException
export function protocolError(code?: string): MicroSdkError
export interface ContextRequestOptions {
  props: MicroProps
  url: string | URL
  options?: RequestInit
  fetchImpl?: typeof fetch
  check?: () => void
  responseType?: 'json' | 'text'
  maxBytes?: number
}
export function requestWithContext<T = unknown>(options: ContextRequestOptions & { responseType: 'text' }): Promise<string>
export function requestWithContext<T = unknown>(options: ContextRequestOptions & { responseType?: 'json' }): Promise<T>
