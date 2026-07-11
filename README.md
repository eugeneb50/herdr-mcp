[![License: AGPL v3](https://img.shields.io/badge/License-AGPLv3-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange)](https://rustup.rs/)
[![herdr-mcp](https://img.shields.io/badge/herdr--mcp-v0.1.0--dev-22c55e)](https://github.com/herdr-mcp)
[![Tests](https://img.shields.io/badge/tests-233%20passing-22c55e)](https://github.com/herdr-mcp)
[![Crates](https://img.shields.io/badge/workspace-4%20crates-22c55e)](https://github.com/herdr-mcp)

# herdr-mcp

MCP (Model Context Protocol) server written in Rust that exposes [herdr](https://herdr.dev) — a terminal-native agent multiplexer — as tools.

**HTTP bridge + interactive web playground** included: run individual tools or chain them into multi-step recipes with variable passing, all from a browser.

- [Quick start](#quick-start)
- [Features](#features)
- [Architecture](#architecture)
- [Documentation](#documentation)
- [Install & build](#install--build)
- [Usage](#usage)
  - [MCP mode](#mcp-mode)
  - [HTTP + web playground](#http--web-playground)
  - [HTTP API](#http-api)
  - [Recipe format](#recipe-format)
  - [Environment](#environment)
- [Tools (49)](#tools-49)
- [Development](#development)
- [Contributing](#contributing)
- [License](#license)

---

## Quick start

```bash
cargo build --release
./target/release/herdr-mcp
```

Requires the `herdr` CLI on `PATH` ([install](https://herdr.dev)).

For the web playground:

```bash
cargo build --release
./target/release/herdr-mcp serve --http 7676 --http-only
# Open http://localhost:7676/
```

---

## Features

- **MCP mode** — plug into any MCP-compatible client (Claude Desktop, Cursor, Claude Code, Continue, OpenCode) to control herdr workspaces, tabs, panes, and agents
- **HTTP bridge** — built-in Axum HTTP server enables browser-based interaction
- **Web playground** — full React UI for exploring and invoking tools, building recipes, and inspecting results
- **49 tools** — discovery, lifecycle, read, write, synchronize, a2a primitives, message-trim, recipe templates, scheduler, and folder-key operations against herdr
- **Recipe engine** — chain multiple tool calls with variable interpolation (`{{ stepId.result.path }}`)
- **Message-trim pipeline** — `caveman` (lossy style) + `pfc1` (lossless Cherokee-syllabary phonetic) compressors for agent-to-agent comms
- **4-crate workspace** — `herdr-mcp-core`, `herdr-mcp-trim`, `herdr-mcp-server`, `herdr-mcp-cli` (233 tests, all passing)
- **No external dependencies** beyond herdr itself — shells out to the CLI via `tokio::process::Command`

---

## Architecture

```mermaid
graph TB
    subgraph MCP[" "]
        direction LR
        AI(["AI Client<br/>(Claude Desktop, Cursor, etc.)"])
        STDIO["herdr-mcp<br/>(stdio JSON-RPC)"]
        AI -- stdin/stdout --> STDIO
    end

    subgraph HTTP[" "]
        direction LR
        B1["Browser<br/>(localhost:5173)"]
        VITE["Vite Dev Server<br/>(proxies /api)"]
        B2["Browser<br/>(localhost:8080)"]
        AXUM["Axum HTTP Bridge<br/>(port 8080)"]
        B1 --> VITE -- proxy --> AXUM
        B2 --> AXUM
    end

    subgraph CORE[" "]
        TOOL["herdr-mcp server.rs<br/>(tool dispatch + recipe engine)"]
        CLI["herdr CLI<br/>(tokio::process::Command)"]
    end

    STDIO --> TOOL
    AXUM --> TOOL
    TOOL --> CLI
    CLI --> DAEMON["herdr daemon<br/>(workspaces, tabs, panes, agents)"]
```

The server is a thin wrapper that shells out to the local `herdr` CLI binary. It supports two transport modes:

- **MCP stdio** — communicates over stdin/stdout using JSON-RPC, compatible with all MCP clients
- **HTTP bridge** — runs alongside the MCP server when `--http <port>` is provided; the web playground communicates over HTTP

**Why shell out instead of speaking the socket protocol directly?** Herdr's wire protocol isn't publicly documented and may change; the CLI is the stable, documented surface. Zero coupling, easy to keep up to date.

### Project structure

```
herdr-mcp/
├── crates/
│   ├── herdr-mcp-core/      # Config system (TOML + env + CLI), error types
│   ├── herdr-mcp-trim/      # Message-trim pipeline (caveman, pfc1, folder keys, dashboard)
│   ├── herdr-mcp-server/    # MCP server, 49 tools, HTTP bridge, recipe engine, event subscriber
│   └── herdr-mcp-cli/       # Binary entrypoint (serve/trim/dashboard/folder-key)
├── src/
│   ├── main.rs              # Legacy monolith binary entrypoint
│   ├── server.rs            # Legacy monolith server: 21+ tool defs, HTTP bridge, recipe engine
│   ├── main.tsx             # React entrypoint (Vite + Tailwind + TypeScript)
│   ├── App.tsx              # HashRouter: landing / docs / playground / trim / variables
│   ├── components/          # Landing page + Documentation + Playground components
│   └── recipes/             # TypeScript recipe types for frontend
├── Cargo.toml               # 4-member workspace — rmcp, axum, clap, tokio
├── package.json             # Vite + React 19 + Tailwind 4 + react-router-dom + @dnd-kit
├── vite.config.ts           # vite-plugin-singlefile, /api proxy → localhost:8080
└── index.html
```

The 4-crate workspace is the canonical home for new code; the `src/` monolith
is legacy. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full
reference.

---

## Documentation

| Document | Description |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Comprehensive architecture reference: data flow, tool reference, HTTP API, recipe engine, trim system, agent registry, persistence, scheduler, frontend, dependencies, build/run, design decisions, known gotchas |
| [docs/PRD.md](docs/PRD.md) | Product requirements document |
| [AGENTS.md](AGENTS.md) | Cross-tool agent instructions for AI coding assistants (conventions, risk tiers, workflow, anti-patterns) |
| [compressorplan.md](compressorplan.md) | Message-trim design plan (caveman + pfc1) |
| [a2a.md](a2a.md) | Agent-to-agent primitive design and usage |
| [TEST_PLAN.md](TEST_PLAN.md) | Test suite specification (233 tests across 4 crates) |
| [herdmcp.toml](herdmcp.toml) | Persistent configuration (HTTP port, data dir, herdr socket) |

---

## Install & build

### Prerequisites

- [Rust](https://rustup.rs/) 1.75+
- [herdr](https://herdr.dev) CLI on `PATH`
- [Node.js](https://nodejs.org/) 20+ (only needed for website development)

### Build from source

```bash
git clone <repo-url>
cd herdr-mcp

# Build the MCP server binary
cargo build --release

# Build the website (optional — single HTML file)
npm install
npm run build
```

### CLI flags

| Flag | Description |
|------|-------------|
| `--http <port>` | Start HTTP bridge on given port |
| `--http-only` | Run HTTP server only (skip MCP stdio transport) |

### Persistent configuration

Create a `herdmcp.toml` file in the working directory to set defaults:

```toml
# herdmcp.toml - Persistent configuration for herdr-mcp
# This file is read from the current working directory when herdr-mcp starts.

[persistent]
# HTTP bridge port (default: 7676). CLI flag --http overrides this.
http_port = 7676

# Data directory for recipes, sessions, trim stats, PFC1 memory (default: ./data)
data_dir = "./data"

# herdr daemon socket path (default: ~/.config/herdr/herdr.sock)
herdr_socket = "~/.config/herdr/herdr.sock"
```

All settings in `herdmcp.toml` are overridden by CLI flags and environment variables.

---

## Usage

### MCP mode

```bash
./target/release/herdr-mcp
```

Add to your MCP client config:

```json
{
  "mcpServers": {
    "herdr-mcp": {
      "command": "/path/to/herdr-mcp"
    }
  }
}
```

By default, this starts the MCP stdio transport **and** the HTTP bridge on port **7676**.

### HTTP + web playground

```bash
# Start server with HTTP bridge on port 7676 (default), skip MCP stdio
./target/release/herdr-mcp serve --http 7676 --http-only
# Open http://localhost:7676/
```

For development with hot-reload on the website:

```bash
# Terminal 1: Rust HTTP server
cargo run --release -- serve --http 7676 --http-only

# Terminal 2: Vite dev server (proxies /api → localhost:7676)
npm run dev

# Open http://localhost:5173/
```

### HTTP API

All endpoints return JSON. See [docs/ARCHITECTURE.md §5](docs/ARCHITECTURE.md#5-http-bridge-api) for the full route reference.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/health` | Health check → `"ok"` |
| `GET` | `/api/tools` | List all 49 tools with JSON schemas |
| `POST` | `/api/tools/:name` | Invoke a tool by name, body is the parameter object |
| `POST` | `/api/recipe` | Execute a multi-step recipe with variable interpolation |
| `GET` | `/api/recipes` | List saved recipes |
| `POST` | `/api/recipes` | Create a recipe |
| `GET` / `PUT` / `DELETE` | `/api/recipes/:id` | Get / update / delete a recipe |
| `POST` | `/api/recipes/:id/run` | Run a recipe by ID |
| `GET` | `/api/trim/status` | Aggregate trim savings (query: `?workspace_id=`) |
| `POST` | `/api/trim/diagnose` | End-to-end trim readiness check |
| `GET` | `/api/variables` | List all variables |

### Recipe format

```json
{
  "name": "optional name",
  "steps": [
    {
      "id": "step1",
      "tool": "list_workspaces",
      "params": {},
      "description": "Get workspaces"
    },
    {
      "id": "step2",
      "tool": "read_pane",
      "params": {
        "pane_id": "{{ step1.result.content[0].workspaces[0].active_tab_id }}",
        "source": "visible"
      },
      "description": "Read first pane"
    }
  ]
}
```

Variables are resolved from previous step results using `{{ stepId.result.path }}` syntax with dot/bracket navigation.

### Environment

| Variable | Default | Description |
|----------|---------|-------------|
| `HERDR_BIN` | `herdr` | Path to herdr CLI binary |
| `RUST_LOG` | `herdr_mcp=info` | Tracing filter |

---

## Tools (49)

Full parameter reference: [docs/ARCHITECTURE.md §4](docs/ARCHITECTURE.md#4-mcp-tool-reference-49-tools-total).

### Discovery (7)
| Tool | Description |
|------|-------------|
| `status` | Get overall herdr server status |
| `list_workspaces` | List all workspaces |
| `list_tabs` | List tabs (optionally by workspace) |
| `list_panes` | List panes (optionally by workspace) |
| `list_agents` | List all detected agents |
| `get_pane` | Get pane details by pane_id or label |
| `get_agent` | Get agent details |

### Lifecycle (5)
| Tool | Description |
|------|-------------|
| `create_workspace` | Create a new workspace |
| `create_tab` | Create a new tab |
| `split_pane` | Split a pane right or down (by pane_id or label) |
| `close_pane` | Close a pane by pane_id or label |
| `start_agent` | Start an agent in a new pane |

### Read (2) / Write (4)
| Tool | Description |
|------|-------------|
| `read_pane` | Read output from a pane (by pane_id or label) |
| `read_agent` | Read output from an agent |
| `send_text` | Send text to a pane (no Enter, by pane_id or label) |
| `send_keys` | Send key presses to a pane (by pane_id or label) |
| `run_command` | Run a command in a pane (text + Enter, by pane_id or label) |
| `send_agent` | Send text to an agent |

### Synchronize (3)
| Tool | Description |
|------|-------------|
| `wait_output` | Wait for specific output in a pane (by pane_id or label) |
| `wait_pane_agent_status` | Wait for a pane's agent status (by pane_id or label) |
| `wait_agent_status` | Wait for an agent status by target |

### A2A primitives (5)
| Tool | Description |
|------|-------------|
| `agent_spawn` | Spawn agent, register role→pane, wait for deps, capture output |
| `agent_message` | Send message with optional trim; honors per-pane policy |
| `agent_read` | Read + store as work product; auto-decompress PFC1 |
| `agent_wait` | Wait for agent by role/pane |
| `agent_list` | List registry agents |

### Session variables (2) / Message-trim (8) / Recipes (3) / Scheduler (4) / Folder-key (4)
| Tool | Description |
|------|-------------|
| `var_get` / `var_set` | Get / set session variable |
| `compress` / `decompress` | PFC1/Caveman trim round-trip |
| `trim_policy_get` / `trim_policy_set` | Read / attach trim policy on agent |
| `trim_eval` / `trim_bench` / `trim_status` / `trim_diagnose` / `trim_summary` / `trim_dashboard_open` | Trim analytics & dashboards |
| `list_templates` / `get_template` / `instantiate_template` | Bundled recipe templates |
| `schedule_recipe` / `list_schedules` / `delete_schedule` / `enable_schedule` | Cron-based recipe scheduling |
| `build_folder_key` / `get_folder_key` / `list_folder_keys` / `decompress_with_folder_key` | Per-folder PFC1 keys |

**Important:** IDs are session-local and may compact when items are closed. Re-read IDs from list commands after structural changes.
All pane-targeting tools accept an optional `label` parameter as an alternative to `pane_id` — the server looks up the label via `herdr pane list` automatically.

---

## Development

See [AGENTS.md](AGENTS.md) for full agent-specific development conventions, build commands, and design notes. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the comprehensive architecture reference.

```bash
# Terminal 1: Rust server with HTTP bridge
cargo run --release -- --http 8080 --http-only

# Terminal 2: Vite dev server
npm run dev

# Full test suite (233 tests across 4 crates)
cargo test --workspace
```

- Rust code lives in `crates/` (4-crate workspace); legacy `src/server.rs` monolith also present
- Website is a single-page React app bundled via `vite-plugin-singlefile` into `dist/index.html`
- 233 tests across 4 crates (all passing): `herdr-mcp-trim` (126), `herdr-mcp-server` (73), `herdr-mcp-core` (24), `herdr-mcp-cli` (10)
- `RUST_LOG` controls tracing verbosity; stdout is reserved for MCP JSON-RPC (stderr for logs)

---

## Contributing

Please read [CONTRIBUTING.md](CONTRIBUTING.md) for details on the code of conduct and the pull request process.

All contributions must be certified via the [Developer Certificate of Origin (DCO)](https://github.com/apps/dco/).

---

## License

This project is licensed under the **GNU Affero General Public License v3.0** — see the [LICENSE](LICENSE) file for details.
