#!/usr/bin/env node
/**
 * dsh-server.mjs — builds deepseek-harness when stale, then starts the web
 * server as a child process.
 *
 * Contract with the Tauri host (dsh-client):
 *   - stdout carries ONLY machine-readable lines:
 *         [status] <message>     progress message shown on the loading page
 *         [error]  <message>     fatal error; the launcher then exits nonzero
 *   - all build / server logs go to files under <client>/logs/ (never stdout).
 *
 * This script never touches deepseek-harness's own code; it only reads and
 * runs it. Env overrides:
 *   DSH_REPO_ROOT     path to the deepseek-harness checkout (else ../deepseek-harness)
 *   DSH_CLIENT_DIR    path to this client (else inferred from this file's location)
 *   DSH_FORCE_REBUILD=1   skip the staleness check and always rebuild
 */

import { spawn } from 'node:child_process'
import { createWriteStream, existsSync, mkdirSync, readdirSync, statSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const CLIENT_ROOT = process.env.DSH_CLIENT_DIR || path.resolve(__dirname, '..')
const REPO_ROOT = process.env.DSH_REPO_ROOT || path.resolve(CLIENT_ROOT, '..', 'deepseek-harness')
const LOG_DIR = path.join(CLIENT_ROOT, 'logs')
const IS_WIN = process.platform === 'win32'

const out = (s) => process.stdout.write(`${s}\n`)
const status = (s) => out(`[status] ${s}`)
const fail = (s) => out(`[error] ${s}`)

// ---------------------------------------------------------------- build check

/** Build artifacts that must exist for `dsh web` to boot. */
const MARKERS = ['apps/cli/lib/bin.js', 'apps/web/dist/index.html']

/** Directories whose mtimes never count as "source changed". */
const SKIP_DIRS = new Set([
  'node_modules', '.git', 'dist', 'lib', 'target', '.turbo', 'coverage', 'build',
])

function isExcluded(p) {
  return (
    p.includes(`${path.sep}node_modules${path.sep}`) ||
    p.endsWith(`${path.sep}node_modules`) ||
    p.includes(`${path.sep}.git${path.sep}`)
  )
}

/** Newest mtime under `dir`, honoring SKIP_DIRS / node_modules / .git. */
function newestMtime(dir) {
  let newest = 0
  const walk = (d) => {
    let entries
    try {
      entries = readdirSync(d, { withFileTypes: true })
    } catch {
      return
    }
    for (const e of entries) {
      const full = path.join(d, e.name)
      if (isExcluded(full)) continue
      if (e.isDirectory()) {
        if (SKIP_DIRS.has(e.name)) continue
        walk(full)
      } else {
        try {
          const t = statSync(full).mtimeMs
          if (t > newest) newest = t
        } catch {
          /* unreadable file — ignore */
        }
      }
    }
  }
  walk(dir)
  return newest
}

function needsBuild() {
  if (process.env.DSH_FORCE_REBUILD === '1' || process.argv.includes('--rebuild')) return true
  const markers = MARKERS.map((m) => path.join(REPO_ROOT, m))
  if (markers.some((m) => !existsSync(m))) return true

  const markerTime = Math.min(...markers.map((m) => statSync(m).mtimeMs))
  const root = REPO_ROOT
  const sources = [
    newestMtime(path.join(root, 'apps')),
    newestMtime(path.join(root, 'packages')),
    newestMtime(path.join(root, 'scripts')),
  ]
  for (const f of ['package.json', 'pnpm-lock.yaml', 'pnpm-workspace.yaml']) {
    const p = path.join(root, f)
    if (existsSync(p)) sources.push(statSync(p).mtimeMs)
  }
  return Math.max(...sources) > markerTime
}

// ---------------------------------------------------------------- process run

/**
 * Run a command to completion, streaming output into `logFile`.
 * Resolves on exit code 0, rejects otherwise.
 */
function run(cmd, args, logFile) {
  return new Promise((resolve, reject) => {
    const fd = createWriteStream(logFile, { flags: 'a' })
    const child = spawn(cmd, args, {
      cwd: REPO_ROOT,
      shell: IS_WIN,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    })
    child.stdout?.on('data', (d) => fd.write(d))
    child.stderr?.on('data', (d) => fd.write(d))
    child.on('error', (e) => {
      fd.end()
      reject(e)
    })
    child.on('close', (code) => {
      fd.end()
      if (code === 0) resolve()
      else reject(new Error(`"${cmd} ${args.join(' ')}" 退出码 ${code}（见 logs/${path.basename(logFile)}）`))
    })
  })
}

/** Locate the pnpm command; on Windows the shim may be off PATH in GUI launches. */
function findPnpm() {
  if (process.env.PNPM_BIN) return process.env.PNPM_BIN
  if (process.env.PNPM_HOME) {
    const cand = path.join(process.env.PNPM_HOME, IS_WIN ? 'pnpm.cmd' : 'pnpm')
    if (existsSync(cand)) return cand
  }
  if (IS_WIN) {
    for (const cand of [
      path.join(process.env.APPDATA || '', 'npm', 'pnpm.cmd'),
      path.join(process.env.LOCALAPPDATA || '', 'pnpm', 'pnpm.cmd'),
    ]) {
      if (existsSync(cand)) return cand
    }
  }
  return 'pnpm'
}

async function ensureBuilt() {
  mkdirSync(LOG_DIR, { recursive: true })
  const buildLog = path.join(LOG_DIR, 'build.log')

  if (!existsSync(path.join(REPO_ROOT, 'node_modules'))) {
    status('安装依赖 (pnpm install)…')
    try {
      await run(findPnpm(), ['install'], buildLog)
    } catch (e) {
      fail(`依赖安装失败: ${e.message}`)
      process.exit(1)
    }
  }

  if (!needsBuild()) {
    status('构建产物是最新的，跳过构建')
    return
  }

  status('构建中 (pnpm run build)… 首次构建约需 1 分钟')
  try {
    await run(findPnpm(), ['run', 'build'], buildLog)
  } catch (e) {
    fail(`构建失败: ${e.message}`)
    process.exit(1)
  }
  status('构建完成')
}

// ---------------------------------------------------------------- server boot

function startServer() {
  status('启动 deepseek-harness (dsh web)…')
  const serverLog = path.join(LOG_DIR, 'server.log')
  const fd = createWriteStream(serverLog, { flags: 'a' })

  // Same invocation as `pnpm dsh web`, but directly on node so the shim and
  // PATH lookups are not needed. tsx resolves from the repo's node_modules.
  const child = spawn('node', ['--import', 'tsx/esm', 'apps/cli/src/bin.ts', 'web'], {
    cwd: REPO_ROOT,
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
  })
  child.stdout?.on('data', (d) => fd.write(d))
  child.stderr?.on('data', (d) => fd.write(d))
  child.on('error', (e) => {
    fail(`无法启动服务器: ${e.message}（node 是否在 PATH 中？）`)
    process.exit(1)
  })
  child.on('close', (code) => {
    fd.end()
    if (code !== 0) fail(`服务器进程退出 (code ${code})`)
    process.exit(code ?? 1)
  })
  return child
}

// ---------------------------------------------------------------- entry

let serverChild = null

function main() {
  if (!existsSync(path.join(REPO_ROOT, 'package.json'))) {
    fail(`未找到 deepseek-harness 仓库: ${REPO_ROOT}（可设置 DSH_REPO_ROOT 环境变量）`)
    process.exit(1)
  }
  ensureBuilt().then(() => {
    serverChild = startServer()
  })
}

for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, () => {
    try { serverChild?.kill() } catch { /* already gone */ }
    process.exit(0)
  })
}

main()
