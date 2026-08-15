/**
 * stata-ai-skill
 *
 * DeepSeek Harness host plugin that starts the native Stata AI Skill service
 * together with DSH and stops it cleanly on shutdown.
 *
 * - Platform-aware binary resolution: macOS arm64 / macOS legacy / Windows
 *   x64 / Windows arm64 (Intel Mac is not supported by the skill).
 * - Reuses an already-running service on the configured port (no double start).
 * - Restarts the service if it exits unexpectedly (bounded backoff).
 * - Graceful shutdown (`/shutdown`) on DSH dispose, with a kill fallback.
 *
 * The Stata AI Skill skill (SKILL.md) keeps working unchanged: it talks to the
 * same localhost HTTP service, which this plugin guarantees is already online
 * whenever DSH is up.
 */

import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export const name = 'stata-ai-skill'

const DEFAULT_PORT = 19522
const DEFAULT_START_TIMEOUT_MS = 30000
const DEFAULT_MAX_RESTARTS = 5
const DEFAULT_RESTART_BACKOFF_MS = 1000
const DEFAULT_SHUTDOWN_GRACE_MS = 5000

/**
 * Resolve the platform binary directory and file name, or null when this
 * platform is not supported by the skill.
 */
function platformBinary() {
  const { platform, arch } = process
  if (platform === 'darwin') {
    if (arch === 'arm64') return { dir: 'macos-arm64', file: 'stata-ai-skill' }
    if (arch === 'x64') return null // Intel Mac unsupported
    return { dir: 'macos', file: 'stata-ai-skill' }
  }
  if (platform === 'win32') {
    if (arch === 'arm64') return { dir: 'windows-arm64', file: 'stata-ai-skill.exe' }
    return { dir: 'windows', file: 'stata-ai-skill.exe' }
  }
  return null
}

/** The packaged binary directory inside this plugin package (bundled skill). */
function packageBinDir() {
  return fileURLToPath(new URL('../skills/stata-ai-skill/bin', import.meta.url))
}

/**
 * Resolve the executable path in order of preference:
 *   1. config.binPath
 *   2. packaged binary inside the bundled skill (skills/stata-ai-skill/bin/<platform>/<file>)
 *   3. $STATA_AI_SKILL_BIN
 *   4. well-known user skill install locations (~/.agents/skills, ~/.dsh/skills)
 *   5. PATH
 * Returns null when the platform is unsupported.
 */
function resolveBinary(config) {
  if (config.binPath) return resolve(config.binPath)
  const pb = platformBinary()
  if (!pb) return null
  const packaged = join(packageBinDir(), pb.dir, pb.file)
  if (existsSync(packaged)) return packaged
  if (process.env.STATA_AI_SKILL_BIN) return process.env.STATA_AI_SKILL_BIN
  const home = homedir()
  for (const root of [
    join(home, '.agents', 'skills', 'stata-ai-skill'),
    join(home, '.dsh', 'skills', 'stata-ai-skill'),
  ]) {
    const candidate = join(root, 'bin', pb.dir, pb.file)
    if (existsSync(candidate)) return candidate
  }
  return pb.file // rely on PATH
}

/** Probe the service /status endpoint. Resolves true when it answers. */
async function probeStatus(port, timeoutMs = 1500) {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), timeoutMs)
  try {
    const response = await fetch(`http://127.0.0.1:${port}/status`, {
      signal: controller.signal,
    })
    if (!response.ok) return false
    const body = await response.json().catch(() => null)
    return body !== null
  } catch {
    return false
  } finally {
    clearTimeout(timer)
  }
}

/** Wait until the service answers /status or the deadline passes. */
async function waitForOnline(port, timeoutMs, log) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (await probeStatus(port)) return true
    await new Promise((resolveSleep) => setTimeout(resolveSleep, 500))
  }
  return probeStatus(port)
}

/** Try a graceful HTTP shutdown, then hard-kill the process if still alive. */
async function shutdownService(port, child, graceMs, log) {
  try {
    await fetch(`http://127.0.0.1:${port}/shutdown`, { method: 'POST' })
    log?.info('sent /shutdown to Stata AI Skill service')
  } catch {
    log?.warn('HTTP shutdown failed, falling back to process kill')
  }
  if (!child) return
  const killer = setTimeout(() => {
    try {
      child.kill()
    } catch {
      /* already gone */
    }
  }, graceMs)
  killer.unref?.()
}

/** The Cordis plugin entry. */
export function apply(ctx, config = {}) {
  const port = config.port ?? DEFAULT_PORT
  const startTimeoutMs = config.startTimeoutMs ?? DEFAULT_START_TIMEOUT_MS
  const maxRestarts = config.maxRestarts ?? DEFAULT_MAX_RESTARTS
  const restartBackoffMs = config.restartBackoffMs ?? DEFAULT_RESTART_BACKOFF_MS
  const shutdownGraceMs = config.shutdownGraceMs ?? DEFAULT_SHUTDOWN_GRACE_MS
  const log = typeof ctx.logger === 'function' ? ctx.logger('stata-ai-skill') : ctx.logger

  let child = null
  let started = false // did THIS instance spawn the process?
  let stopping = false
  let restartCount = 0
  let restartTimer = null
  let online = false

  async function ensureRunning() {
    if (stopping) return
    if (online) return

    // Already running outside DSH? Reuse it.
    if (await probeStatus(port)) {
      online = true
      restartCount = 0
      log?.info(`Stata AI Skill service already online on port ${port} — reusing it`)
      return
    }

    const binary = resolveBinary(config)
    if (!binary) {
      log?.error('Stata AI Skill does not support this platform (Intel Mac is not supported) — not starting the service')
      return
    }

    log?.info(`starting Stata AI Skill service: ${binary} serve --port ${port}`)
    const proc = spawn(binary, ['serve', '--port', String(port)], {
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
      detached: false,
    })
    child = proc
    started = true

    proc.stdout?.on('data', (chunk) => log?.debug?.('[stata-ai-skill] %s', String(chunk).trimEnd()))
    proc.stderr?.on('data', (chunk) => log?.debug?.('[stata-ai-skill] %s', String(chunk).trimEnd()))

    proc.on('error', (error) => {
      log?.error(`failed to start Stata AI Skill service: ${error.message}`)
    })

    proc.on('exit', (code, signal) => {
      const wasOnline = online
      online = false
      child = null
      if (stopping) {
        log?.info('Stata AI Skill service stopped')
        return
      }
      log?.warn(`Stata AI Skill service exited (code=${code}, signal=${signal})`)
      if (wasOnline || code === 0) restartCount = 0 // clean/served exit resets the budget
      if (restartCount < maxRestarts) {
        restartCount += 1
        const delay = restartBackoffMs * 2 ** (restartCount - 1)
        log?.info(`restarting in ${delay}ms (attempt ${restartCount}/${maxRestarts})`)
        restartTimer = setTimeout(() => {
          restartTimer = null
          void ensureRunning()
        }, delay)
      } else {
        log?.error(`gave up restarting Stata AI Skill service after ${maxRestarts} attempts`)
      }
    })

    if (await waitForOnline(port, startTimeoutMs, log)) {
      online = true
      restartCount = 0
      log?.info(`Stata AI Skill service is online on port ${port}`)
    } else {
      log?.warn(`Stata AI Skill service did not answer /status within ${startTimeoutMs}ms — leaving the process running and retrying on demand`)
    }
  }

  // Start with the host; clean up when the host shuts down.
  ctx.effect(() => {
    void ensureRunning()
    return () => {
      stopping = true
      if (restartTimer) clearTimeout(restartTimer)
      // Only manage what we spawned; a pre-existing service stays untouched.
      if (started && child) {
        void shutdownService(port, child, shutdownGraceMs, log)
      }
    }
  })
}
