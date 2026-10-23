import assert from 'node:assert/strict'
import test from 'node:test'
import { createQiankunLifecycle } from '../src/qiankun-lifecycle.mjs'

const context = (routePath = '/micro/work-orders') => ({ signal: new AbortController().signal, routePath })
function fixture() {
  const calls = [], target = {}, container = { querySelector: selector => { assert.equal(selector, '[data-work-orders-mount]'); return target } }
  let bridge
  const lifecycle = createQiankunLifecycle(value => {
    bridge = value
    return { mount: (...args) => calls.push(['mount', ...args]), update: props => calls.push(['update', props]), unmount: () => calls.push(['unmount']) }
  })
  return { lifecycle, calls, container, target, get bridge() { return bridge } }
}

test('qiankun lifecycle preserves route updates and sends only props.platform to the business SDK', async () => {
  const f = fixture(), first = context(), next = context('/micro/work-orders/tickets/123')
  await f.lifecycle.bootstrap()
  await f.lifecycle.mount({ name: 'injected-by-qiankun', container: f.container, platform: first, services: {} })
  await f.lifecycle.update({ name: 'injected-by-qiankun', container: f.container, platform: next })
  assert.deepEqual(f.calls, [['mount', f.target, first], ['update', next]])
  await f.lifecycle.unmount(); await f.lifecycle.unmount()
  await f.lifecycle.update({ platform: next })
  await f.lifecycle.mount({ container: f.container, platform: next })
  assert.deepEqual(f.calls.map(call => call[0]), ['mount', 'update', 'unmount', 'mount'])
})

test('CSV download remains a host-authorized service, follows context updates, and is unavailable after disposal', async () => {
  const f = fixture(), calls = [], payload = { resource: 'ticket', filename: '工单.csv', content: 'a,b' }
  await f.lifecycle.mount({ container: f.container, platform: context(), services: { downloadCsv: value => calls.push(['first', value]) } })
  await f.bridge.downloadCsv(payload)
  await f.lifecycle.update({ platform: context(), services: { downloadCsv: value => calls.push(['next', value]) } })
  await f.bridge.downloadCsv(payload)
  assert.deepEqual(calls, [['first', payload], ['next', payload]])
  await f.lifecycle.unmount()
  assert.throws(() => f.bridge.downloadCsv(payload), /已授权/)
})

test('missing and cancelled contexts never mount and mounting without a host download service cannot download', async () => {
  const f = fixture(), cancelled = context(); cancelled.signal = AbortSignal.abort()
  await assert.rejects(f.lifecycle.mount({ container: f.container }), /props.platform/)
  await f.lifecycle.mount({ container: f.container, platform: cancelled })
  assert.deepEqual(f.calls, [])
  await f.lifecycle.mount({ container: f.container, platform: context() })
  assert.throws(() => f.bridge.downloadCsv({}), /已授权/)
})
