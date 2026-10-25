import assert from 'node:assert/strict'
import test from 'node:test'
import { newId, validId } from '../uuid.mjs'
import { requestId } from '../observability.mjs'

const invalidIds = [
  '00000000-0000-0000-0000-000000000000', // nil
  '0199fe00-0000-4000-8000-000000000001', // legacy v4
  '0199fe00-0000-7000-0000-000000000001', // NCS variant
  '0199fe00-0000-7000-c000-000000000001', // Microsoft variant
  '0199fe00-0000-7000-f000-000000000001', // reserved variant
  '0199fe00000070008000000000000001', null, {}, [],
]

test('entity IDs require canonical UUIDv7 with the RFC variant', () => {
  const id = newId()
  assert.equal(validId(id), true)
  assert.equal(validId(id.toUpperCase()), true)
  for (const value of invalidIds) assert.equal(validId(value), false)
})

test('request IDs normalize valid v7 and replace all other inbound values', () => {
  const id = newId()
  assert.equal(requestId(id.toUpperCase()), id)
  for (const value of [...invalidIds, undefined, 'untrusted-header', [id, id]]) {
    const replacement = requestId(value)
    assert.equal(validId(replacement), true)
    assert.notEqual(replacement, value)
  }
})
