# herdr-mcp — AGENTS.md

## Build & dev

```bash
cargo build --release          # binary → target/release/herdr-mcp
npm run build                  # website → dist/index.html (single-file via vite-plugin-singlefile)
npm run dev                    # Vite dev server, proxies /api → localhost:8080
```

### Dev workflow

```bash
# Terminal 1: Rust server with HTTP bridge
cargo run --release -- --http 8080 --http-only

# Terminal 2: Vite dev server
npm run dev
# Open http://localhost:5173/
```

`--http-only` skips MCP stdio. Omit it to run both stdio + HTTP.

## Architecture

- **Two projects in one source tree**: Rust MCP server (`src/main.rs` + `src/server.rs`) and Vite+React+Tailwind website (`src/main.tsx` → `src/App.tsx` → `src/components/*`)
- **`server.rs`** contains everything: 21 tool definitions, ServerHandler impl, HTTP bridge (Axum), recipe engine, and CLI helpers (`herdr_cli`, `run_herdr_json`, `run_herdr_text`). No `herdr.rs` module.
- **Stdout = MCP JSON-RPC**, stderr = tracing/logs. Never print to stdout from server code.
- **Website is single-file** — vite-plugin-singlefile inlines all JS/CSS into `dist/index.html`. Adding external assets (images, fonts) needs explicit inline handling.
- **No test framework** — zero tests.

## Runtime

- Requires `herdr` CLI on `PATH` (https://herdr.dev). Local path: `/home/producer32/.local/bin/herdr`.
- `HERDR_BIN` env var overrides the binary path (default: `"herdr"`). See `server.rs:454`.
- `RUST_LOG` for tracing filtering (default: `herdr_mcp=info`).

## Design notes

- Every tool shells out via `tokio::process::Command` with real argv — no shell injection.
- `run_command` is atomic (text + Enter) — prefer over `send_text` + `send_keys Enter`.
- `start_agent` appends `--` before the agent name to prevent herdr from consuming agent flags.
- CLI errors returned as MCP `isError: true` content (not protocol errors).
- IDs are session-local and may compact — re-read from list commands after structural changes.
- Recipe engine supports variable interpolation: `{{ stepId.result.path }}` with dot/bracket navigation.

## Website specifics

- Tailwind v4 (`@import "tailwindcss"` in `index.css`), not v3.
- TypeScript strict mode with `noUnusedLocals`/`noUnusedParameters` — will fail build on unused imports/vars.
- Single-page app via HashRouter: routes `/`, `/docs`, `/playground`, `/variables`.
- No lint or typecheck npm scripts defined — `tsc --noEmit` is the typecheck command if needed.
- `.gitignore` ignores `Cargo.lock` (unusual for binaries; it's tracked in git only if you add it explicitly).

## Dependencies

- **Rust**: `rmcp` 1.7 (server, transport-io, macros), `tokio` (full), `axum` 0.8, `tower-http` 0.6 (cors), `clap` 4, `serde`/`serde_json`, `schemars`, `anyhow`, `tracing`/`tracing-subscriber`, `regex`
- **Website**: React 19, Vite 7, Tailwind CSS 4, TypeScript 5.9, react-router-dom 7, @dnd-kit core+sortable, clsx, tailwind-merge
