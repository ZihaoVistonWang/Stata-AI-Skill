// End-to-end test: mount the plugin like DSH would, wait for the service,
// then run a real Stata command through the HTTP API the skill uses.
import { apply } from '../lib/index.js'

const disposables = []
const fakeCtx = {
  logger: (...args) => console.log('[logger]', ...args),
  effect(fn) { disposables.push(fn()) },
  on() {},
  get() { return undefined },
}

const PORT = 19524
console.log('--- 1. apply(plugin) like DSH boot ---')
apply(fakeCtx, { port: PORT, startTimeoutMs: 30000 })

console.log('--- 2. waiting for service online ---')
const deadline = Date.now() + 30000
let online = false
while (Date.now() < deadline) {
  try {
    const res = await fetch(`http://127.0.0.1:${PORT}/status`)
    if (res.ok) { online = true; break }
  } catch {}
  await new Promise((r) => setTimeout(r, 500))
}
console.log('service online:', online)
if (!online) process.exit(1)

console.log('--- 3. run real Stata command via /execute ---')
const execRes = await fetch(`http://127.0.0.1:${PORT}/execute`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ code: 'display "e2e-ok "' + '"' + 'plugin-works"', timeout: 60 }),
})
const exec = await execRes.json()
console.log('execute success:', exec.success, '| output:', exec.output?.trim?.() ?? exec.output, '| returnCode:', exec.returnCode)

console.log('--- 4. dispose (DSH shutdown) ---')
for (const dispose of disposables.splice(0).reverse()) {
  if (typeof dispose === 'function') await dispose()
}
await new Promise((r) => setTimeout(r, 1200))
let gone = true
try { await fetch(`http://127.0.0.1:${PORT}/status`); gone = false } catch {}
console.log('service stopped after dispose:', gone)

const ok = online && exec.success && String(exec.output ?? '').includes('plugin-works') && gone
console.log(ok ? '=== E2E PASS ===' : '=== E2E FAIL ===')
process.exit(ok ? 0 : 2)
