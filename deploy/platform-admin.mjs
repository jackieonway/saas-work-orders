#!/usr/bin/env node
import { run, SafeAdminError } from './platform-admin-core.mjs';

try {
  const result = await run(process.argv.slice(2), process.env);
  console.log(typeof result === 'string' ? result : JSON.stringify(result, null, 2));
} catch (error) {
  // An API response, parser exception or URL can contain credentials. Never render it.
  const result = { ok: false, category: error instanceof SafeAdminError ? error.category : 'INTERNAL_ERROR' };
  if (error instanceof SafeAdminError && error.status !== undefined) result.status = error.status;
  console.error(JSON.stringify(result));
  process.exitCode = 1;
}
