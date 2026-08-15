// Smoke test for dsh-stata-ai-skill plugin: mounts the plugin on a fake
// Cordis-like ctx, verifies it spawns the service, waits for /status,
// then verifies clean shutdown via the effect disposer.
import { apply } from '../lib/index.js'

const disposables = []
const fakeCtx = {
  logger: (...args) => console.log('[logger]', ...args),
  effect(fn) {
    disposables.push(fn())
  },
  on() {},
  get() { return undefined },
}

async function waitForStatus(port, timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/status`)
      if (res.ok) return true
    } catch {}
    await new Promise((r) => setTimeout(r, 500))
  }
  return false
}

const PORT = 19523
console.log('--- apply(plugin, {port: 19523}) ---')
apply(fakeCtx, { port: PORT, startTimeoutMs: 20000 })
console.log('--- waiting for service on', PORT, '---')
const online = await waitForStatus(PORT)
console.log('service online:', online)
if (!online) process.exit(1)

const status = await (await fetch(`http://127.0.0.1:${PORT}/status`)).json()
console.log('status.config.port:', status.config?.port, '| sessionActive:', status.sessionActive)

console.log('--- running disposer (DSH shutdown) ---')
for (const dispose of disposables.splice(0).reverse()) {
  if (typeof dispose === 'function') await dispose()
}
await new Promise((r) => setTimeout(r, 1000))
const gone = !(await waitForStatus(PORT, 3000))
console.log('service stopped after dispose:', gone)
process.exit(gone ? 0 : 2)
