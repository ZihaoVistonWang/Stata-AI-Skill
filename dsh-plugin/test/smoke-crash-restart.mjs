// Crash-restart test: plugin spawns the service, we hard-kill the child
// process to simulate a crash, then verify the plugin restarts it and the
// service comes back online.
import { apply } from '../lib/index.js'

const disposables = []
const fakeCtx = {
  logger: (...args) => console.log('[logger]', ...args),
  effect(fn) { disposables.push(fn()) },
  on() {},
  get() { return undefined },
}

const PORT = 19525
console.log('--- 1. apply(plugin) ---')
apply(fakeCtx, { port: PORT, startTimeoutMs: 30000, restartBackoffMs: 500 })

async function waitOnline(timeoutMs) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`http://127.0.0.1:${PORT}/status`)
      if (res.ok) return true
    } catch {}
    await new Promise((r) => setTimeout(r, 300))
  }
  return false
}

console.log('--- 2. wait for service online ---')
const online1 = await waitOnline(30000)
console.log('service online:', online1)
if (!online1) process.exit(1)

// Find the service PID listening on PORT
const { execSync } = await import('node:child_process')
const lsof = execSync(`lsof -tiTCP:${PORT} -sTCP:LISTEN`, { encoding: 'utf8' }).trim()
const pid = Number(lsof.split('\n')[0])
console.log('--- 3. service PID:', pid, '— hard-killing it (simulated crash) ---')
process.kill(pid, 'SIGKILL')

// Give the plugin a moment to notice the exit, then wait for restart
console.log('--- 4. waiting for plugin to restart the service ---')
const online2 = await waitOnline(60000)
console.log('service back online after crash:', online2)
if (!online2) process.exit(2)

// Verify it's a NEW process (different PID)
const lsof2 = execSync(`lsof -tiTCP:${PORT} -sTCP:LISTEN`, { encoding: 'utf8' }).trim()
const pid2 = Number(lsof2.split('\n')[0])
console.log('--- 5. new PID:', pid2, '| restarted as new process:', pid2 !== pid)

console.log('--- 6. dispose (DSH shutdown) ---')
for (const dispose of disposables.splice(0).reverse()) {
  if (typeof dispose === 'function') await dispose()
}
await new Promise((r) => setTimeout(r, 1200))
let gone = true
try { await fetch(`http://127.0.0.1:${PORT}/status`); gone = false } catch {}
console.log('service stopped after dispose:', gone)

const ok = online1 && online2 && pid2 !== pid && gone
console.log(ok ? '=== CRASH-RESTART PASS ===' : '=== CRASH-RESTART FAIL ===')
process.exit(ok ? 0 : 3)
