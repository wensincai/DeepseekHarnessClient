# dsh-client — DeepSeek Harness Desktop Client

A thin desktop client for [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness),
built with Tauri 2 (Rust + WebView2). It spawns the harness web server as a child
process, shows a loading page while dependencies install and artifacts build on
demand, then loads `http://127.0.0.1:3080` in an embedded browser window.

**Non-invasive** — never modifies the `deepseek-harness` checkout.

## Highlights

- **Builds on demand** — reinstalls/rebuilds only when sources are newer than the
  artifacts; daily startup is instant.
- **Progress feedback** — the loading page shows live build/startup status.
- **Clean teardown** — closing the window kills the whole server process tree.
- **Env overrides** — `DSH_REPO_ROOT`, `DSH_FORCE_REBUILD=1`, `DSH_CLIENT_DIR`.

## Requirements

- Node.js + npm (for this client)
- Rust toolchain (for the Tauri shell; first compile takes 5–15 min)
- pnpm (used to build the harness checkout)
- A `deepseek-harness` source checkout (default: `../deepseek-harness`, override with `DSH_REPO_ROOT`)

## Quick start

```bash
npm install          # first time only
npm run dev          # dev mode (first cargo compile: 5–15 min)
npm run build:portable   # standalone exe: src-tauri/target/release/dsh-client.exe
```

## 工作原理（How it works）

```
dsh-client (Tauri, WebView2)
  │
  ├─ 1. spawn scripts/dsh-server.mjs (node)
  │        ├─ artifacts stale / missing → pnpm install + pnpm run build
  │        └─ start: node --import tsx/esm apps/cli/src/bin.ts web   (= `pnpm dsh web`)
  │
  ├─ 2. Loading page polls server_status, showing build/startup progress
  │
  ├─ 3. When http://127.0.0.1:3080 answers → WebView navigates there
  │
  └─ 4. Window closed → taskkill /T kills the whole process tree
```

Build/server logs are written to `logs/`.

## 常用命令（Commands）

| Command | Purpose |
|---|---|
| `npm run dev` | Run in dev mode |
| `npm run build` | Build NSIS installer |
| `npm run build:portable` | Standalone exe only |
| `npm run icons` | Regenerate window icons from `tools/deepseek-whale.svg` |
| `python tools/gen-icon.py` | Regenerate the icon source SVG (edit `COLOR` in the script to re-tint) |

## 配置（Env overrides）

| Variable | Purpose |
|---|---|
| `DSH_REPO_ROOT` | Path to the deepseek-harness checkout (default `<client>/../deepseek-harness`) |
| `DSH_FORCE_REBUILD=1` | Always rebuild before starting |
| `DSH_CLIENT_DIR` | Client root (rarely needed) |

## 目录结构（Layout）

```
client/
├── client-ui/            # Loading page (shown before the server is ready)
├── scripts/dsh-server.mjs# Launcher: build-if-stale + dsh web + status lines
├── tools/deepseek-whale.svg # Icon source: DeepSeek whale logo, bright green
├── tools/gen-icon.py     # Regenerates the icon source SVG (change COLOR to re-tint)
├── src-tauri/            # Tauri (Rust) shell
│   └── src/
│       ├── lib.rs        # Tauri entry, server_status command
│       └── server.rs     # child process, readiness probe, navigation, teardown
└── logs/                 # build.log / server.log (created at runtime)
```

## License

MIT
