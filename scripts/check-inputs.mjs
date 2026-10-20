import { readFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
const root = new URL('../', import.meta.url)
const lock = JSON.parse(readFileSync(new URL('dependencies.lock.json', root)))
for (const [file, expected] of Object.entries(lock.files)) {
  if (file.startsWith('/') || file.split('/').includes('..')) throw new Error('Unsafe dependency path')
  const actual = createHash('sha256').update(readFileSync(new URL(file, root))).digest('hex')
  if (actual !== expected) throw new Error(`Pinned dependency changed: ${file}`)
}
console.log(`Verified ${Object.keys(lock.files).length} pinned dependency files; SDK ${lock.sdkVersion}`)
