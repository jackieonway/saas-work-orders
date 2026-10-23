export type BrowserCapabilityMode = 'client'
export interface BrowserCapabilityResult {
  readonly mode: BrowserCapabilityMode
  readonly supported: boolean
  readonly missing: readonly string[]
}
export function detectBrowserCapabilities(mode?: BrowserCapabilityMode, environment?: unknown): BrowserCapabilityResult
export function assertBrowserCapabilities(mode?: BrowserCapabilityMode, environment?: unknown): BrowserCapabilityResult
