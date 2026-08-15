// Verify the "reuse existing service" path: with a service already on the
// port, the plugin must NOT spawn a second process.
import { apply } from '../lib/index.js'

const disposables = []
const fakeCtx = {
  logger: (...args) => console.log('[logger]', ...args),
  effect(fn) { disposables.push(fn()) },
  on() {},
  get() { return undefined },
}

const PORT = 19522
console.log('--- apply(plugin, {port: 19522}) with existing service ---')
apply(fakeCtx, { port: PORT, startTimeoutMs: 10000 })
await new Promise((r) => setTimeout(r, 4000))
// If a second process had been spawned it would have failed to bind; the
// original service must still answer and only ONE process owns the port.
const res = await fetch(`http://127.0.0.1:${PORT}/status`)
console.log('status after apply:', res.ok)
console.log('--- dispose (must NOT shut down a pre-existing service) ---')
for (const dispose of disposables.splice(0).reverse()) {
  if (typeof dispose === 'function') await dispose()
}
await new Promise((r) => setTimeout(r, 1000))
const stillOnline = await fetch(`http://127.0.0.1:${PORT}/status`).then((r) => r.ok).catch(() => false)
console.log('pre-existing service still online after dispose:', stillOnline)
process.exit(stillOnline ? 0 : 2)
