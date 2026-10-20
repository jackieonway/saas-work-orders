import test from 'node:test'
import assert from 'node:assert/strict'
import { existsSync, readFileSync } from 'node:fs'
const root = new URL('../../', import.meta.url)
test('Docker source COPY inputs exist inside the independent repository', () => {
  for (const dockerfile of ['frontend/Dockerfile', 'backend/Dockerfile']) {
    const text = readFileSync(new URL(dockerfile, root), 'utf8')
    for (const line of text.split(/\r?\n/)) {
      if (!line.startsWith('COPY ') || line.startsWith('COPY --from=')) continue
      const context = new URL(dockerfile.startsWith('frontend/') ? 'frontend/' : 'backend/', root)
      const sources = line.slice(5).trim().split(/\s+/).slice(0, -1)
      for (const source of sources) {
        assert.ok(!source.includes('..') && !source.includes('$') && !source.startsWith('/'), source)
        assert.ok(existsSync(new URL(source, context)), `${dockerfile}: missing ${source}`)
      }
    }
  }
})
