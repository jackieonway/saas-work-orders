import assert from 'node:assert/strict'
import test from 'node:test'
import { createLazyLifecycle } from '../src/lifecycle.mjs'

function deferred() { let resolve; const promise = new Promise(done => { resolve = done }); return { promise, resolve } }
function fixture(loader) {
  const calls = [], bridge = {}, application = {
    mount: (container, props) => { calls.push(['mount', container, props]) },
    update: props => { calls.push(['update', props]) },
    unmount: () => { calls.push(['unmount']) },
  }
  const module = { createLifecycle(value) { assert.equal(value, bridge); calls.push(['create']); return application } }
  return { calls, module, lifecycle: createLazyLifecycle(loader || (() => module), () => bridge) }
}
const props = signal => ({ signal })

test('SDK lifecycle is returned synchronously without starting the dynamic application load', async () => {
  let loads = 0
  const f = fixture(() => { loads++; return f.module })
  assert.equal(typeof f.lifecycle.mount, 'function')
  assert.equal(loads, 0)
  const context = props(new AbortController().signal), container = {}
  await f.lifecycle.mount(container, context)
  await f.lifecycle.update(context)
  assert.equal(loads, 1)
  assert.deepEqual(f.calls.map(call => call[0]), ['create', 'mount', 'update'])
  assert.equal(f.calls[1][1], container)
  assert.equal(f.calls[1][2], context)
})

test('a context cancelled while loading cannot cause a late Vue mount; fresh SDK update performs the first real mount', async () => {
  const imported = deferred(), f = fixture(() => imported.promise)
  const cancelled = new AbortController(), current = new AbortController()
  const container = {}, first = f.lifecycle.mount(container, props(cancelled.signal))
  cancelled.abort()
  imported.resolve(f.module)
  await first
  assert.equal(f.calls.some(call => call[0] === 'mount'), false)
  const context = props(current.signal)
  await f.lifecycle.update(context)
  assert.equal(f.calls.filter(call => call[0] === 'create').length, 1)
  assert.equal(f.calls.filter(call => call[0] === 'mount').length, 1)
  assert.equal(f.calls.at(-1)[1], container)
  assert.equal(f.calls.at(-1)[2], context)
})

test('disposal during an outstanding import prevents factory creation and every later mount', async () => {
  const imported = deferred(), f = fixture(() => imported.promise)
  const context = props(new AbortController().signal)
  const mounting = f.lifecycle.mount({}, context)
  await f.lifecycle.unmount()
  imported.resolve(f.module)
  await mounting
  await f.lifecycle.mount({}, context)
  await f.lifecycle.update(context)
  await f.lifecycle.unmount()
  assert.deepEqual(f.calls, [])
})

test('the loaded application is unmounted once and cannot receive updates after disposal', async () => {
  const f = fixture(), context = props(new AbortController().signal)
  await f.lifecycle.mount({}, context)
  await f.lifecycle.unmount()
  await f.lifecycle.unmount()
  await f.lifecycle.update(context)
  assert.deepEqual(f.calls.map(call => call[0]), ['create', 'mount', 'unmount'])
})

test('an already cancelled mount does not start loading any business code', async () => {
  let loads = 0
  const f = fixture(() => { loads++; return f.module }), source = new AbortController()
  source.abort()
  await f.lifecycle.mount({}, props(source.signal))
  assert.equal(loads, 0)
  assert.deepEqual(f.calls, [])
})
