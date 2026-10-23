import { spawnSync } from 'node:child_process'
import { constants, copyFileSync, existsSync, lstatSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createStaticBuildPlan } from '../deploy/build-static.mjs'
import { knownFields } from '../deploy/gateway-policy.mjs'
import { readPublicConfig } from '../vite.config.mjs'
import { checkRelease, releaseFiles } from './release-check.mjs'

const appRoot = fileURLToPath(new URL('../', import.meta.url))
const defaultRoot = path.join(appRoot, 'deploy', 'micro-apps')
const within = (root, target) => {
  const relative = path.relative(root, target)
  return relative && !path.isAbsolute(relative) && relative !== '..' && !relative.startsWith('..' + path.sep)
}

function rejectSymlinkAncestors(target) {
  let current = path.resolve(target)
  while (true) {
    if (existsSync(current) && lstatSync(current).isSymbolicLink()) throw new Error('Candidate paths must not use symlinks or junctions')
    const parent = path.dirname(current)
    if (parent === current) break
    current = parent
  }
}

export async function createReleaseCandidate(input, { candidateRoot = defaultRoot, local = false } = {}) {
  // The existing renderer is the authority for the closed, four-field public
  // configuration. Candidate generation never edits a running nginx or host.
  const publicInput = { appCode: 'work-orders', ...input }
  if (publicInput.appCode !== 'work-orders') throw new Error('This repository only releases work-orders')
  if (typeof local !== 'boolean') throw new Error('The local candidate flag must be a boolean')
  let plan
  if (local) {
    knownFields(publicInput, ['appCode', 'version', 'hostOrigin', 'apiBase'])
    if (typeof publicInput.version !== 'string' || !publicInput.version || typeof publicInput.apiBase !== 'string' || !publicInput.apiBase) {
      throw new Error('Local candidates require an explicit version, host origin and API base')
    }
    const host = new URL(publicInput.hostOrigin)
    const api = new URL(publicInput.apiBase, host)
    const loopback = url => url.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)
    if (!loopback(host) || host.origin !== publicInput.hostOrigin || !loopback(api)) throw new Error('--local permits only exact loopback HTTP origins and local API paths')
    readPublicConfig({ version: publicInput.version, origins: [publicInput.hostOrigin], 'api-base': publicInput.apiBase })
    plan = { args: ['scripts/build.mjs', '--version', publicInput.version, '--allowed-host-origin', publicInput.hostOrigin, '--api-base', publicInput.apiBase] }
  } else {
    plan = createStaticBuildPlan(publicInput)
  }
  const root = path.resolve(candidateRoot), temporary = realpathSync(os.tmpdir())
  rejectSymlinkAncestors(root)
  if (root !== path.resolve(defaultRoot) && !within(temporary, root)) throw new Error('Candidate root must be the local deploy directory or a test temporary directory')
  const destination = path.join(root, 'work-orders', 'versions', publicInput.version)
  rejectSymlinkAncestors(destination)
  if (existsSync(destination)) throw new Error(`Immutable candidate already exists; refusing to overwrite: ${destination}`)
  const buildRoot = realpathSync(mkdtempSync(path.join(temporary, 'work-orders-release-candidate-')))
  try {
    const output = path.join(buildRoot, 'dist')
    const built = spawnSync(process.execPath, [...plan.args, '--out-dir', output], {
      cwd: appRoot, encoding: 'utf8', windowsHide: true, timeout: 120000,
    })
    if (built.status !== 0) throw new Error(`Candidate build failed (${built.status}):\n${built.stdout || ''}${built.stderr || ''}`)
    const expectedPublicConfig = { version: publicInput.version, allowedHostOrigins: [publicInput.hostOrigin], apiBase: publicInput.apiBase }
    const checked = await checkRelease(output, { expectedVersion: publicInput.version, expectedPublicConfig })
    const staticDir = path.join(destination, 'html')
    mkdirSync(path.dirname(destination), { recursive: true })
    rejectSymlinkAncestors(destination)
    // Exclusive mkdir is the reservation. A second process cannot overwrite a
    // version, including an empty or incomplete directory. The completion
    // manifest is written last; this directory is never an active deployment.
    mkdirSync(destination)
    mkdirSync(staticDir)
    for (const file of releaseFiles(output)) {
      const target = path.join(staticDir, file.path)
      mkdirSync(path.dirname(target), { recursive: true })
      copyFileSync(path.join(output, file.path), target, constants.COPYFILE_EXCL)
    }
    if (plan.nginx) writeFileSync(path.join(destination, 'nginx.conf'), plan.nginx, { flag: 'wx', mode: 0o644 })
    writeFileSync(path.join(destination, 'public-config.json'), JSON.stringify(publicInput, null, 2) + '\n', { flag: 'wx', mode: 0o644 })
    const report = await checkRelease(staticDir, { expectedVersion: publicInput.version, expectedPublicConfig })
    if (report.inventorySha256 !== checked.inventorySha256) throw new Error('Candidate copy differs from checked build')
    const revision = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: appRoot, encoding: 'utf8', windowsHide: true })
    const status = spawnSync('git', ['status', '--porcelain', '--untracked-files=normal'], { cwd: appRoot, encoding: 'utf8', windowsHide: true })
    const metadata = {
      schemaVersion: 1, state: 'local-candidate', localCandidate: local, deployed: false,
      purpose: local ? 'loopback-development-validation' : 'production-review-candidate',
      application: 'work-orders', version: publicInput.version,
      runtime: 'qiankun', format: 'umd', sdkVersion: report.sdkVersion, contractVersion: '2.0',
      publicConfig: publicInput, staticDirectory: 'html', nginxConfiguration: plan.nginx ? 'nginx.conf' : null,
      suggestedEntryPath: `/sub-apps/work-orders/versions/${publicInput.version}/index.html`,
      sourceRevision: revision.status === 0 ? revision.stdout.trim() : null,
      sourceWorkingTree: status.status === 0 ? Boolean(status.stdout.trim()) : null,
      sourceSha256: report.sourceSha256, releaseInventorySha256: report.inventorySha256,
      files: releaseFiles(destination),
    }
    writeFileSync(path.join(destination, 'candidate.json'), JSON.stringify(metadata, null, 2) + '\n', { flag: 'wx', mode: 0o644 })
    return { directory: destination, staticDirectory: staticDir, metadata }
  } finally {
    if (!within(temporary, buildRoot) || path.dirname(buildRoot) !== temporary ||
      !path.basename(buildRoot).startsWith('work-orders-release-candidate-') || realpathSync(buildRoot) !== buildRoot) {
      throw new Error('Refusing to remove an unverified temporary build directory')
    }
    rmSync(buildRoot, { recursive: true, force: true })
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const input = {}, args = process.argv.slice(2)
  let local = false
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === '--local') {
      if (local) throw new Error('Duplicate candidate option: --local')
      local = true
      continue
    }
    const name = args[index], value = args[index + 1]
    if (!value || !['--version', '--host-origin', '--api-base'].includes(name)) throw new Error('Usage: npm run release:candidate -- [--local] --version <new-version> --host-origin <origin> --api-base <public-api-base>')
    const field = { '--version': 'version', '--host-origin': 'hostOrigin', '--api-base': 'apiBase' }[name]
    if (Object.hasOwn(input, field)) throw new Error(`Duplicate candidate option: ${name}`)
    input[field] = value
    index += 1
  }
  console.log(JSON.stringify(await createReleaseCandidate(input, { local }), null, 2))
}
