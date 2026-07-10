# herdr — Product Requirements Document
Version: 1.0 | Status: Draft | Date: 2026-06-21
Author: AI Agent | Reviewer: Producer32

---

## 1. Executive Summary

**herdr** is a Rust-based core library with a WASM toolkit that exposes the herdr terminal multiplexer as programmable tools via MCP (Model Context Protocol) and HTTP. It provides 21 tools for workspace/pane/agent management, a recipe engine for chaining operations with variable interpolation, and a portable WASM package consumable by any frontend (React, Leptos, TUI, VS Code extension, Raycast workflow). The core metric: **reduce "write a herdr automation" from 200+ lines of custom scripting to <20 lines of recipe JSON or visual canvas nodes.**

---

## 2. Problem Statement

### 2.1 Current State
- herdr users manage workspaces/tabs/panes/agents via CLI (`herdr workspace create`, `herdr pane split`, `herdr agent start`, etc.)
- Automating multi-step workflows requires shell scripts with fragile `pane_id` parsing, manual `jq` filtering, and no variable passing between steps
- No visual builder exists — users write recipes as raw JSON with `{{stepId.result.path}}` templates
- Frontend (React playground) is coupled to the Rust binary via HTTP — not reusable, not portable, not embeddable
- MCP server is a single 869-line `server.rs` with no library boundary — can't be used from other Rust code or WASM

### 2.2 Pain Points
1. **Brittle automation**: Shell scripts break when `pane_id` compacts after closing panes
2. **No variable passing**: Can't use output of `list_panes` as input to `split_pane` without manual copy-paste
3. **Single frontend**: React playground only — no TUI, no VS Code, no Raycast, no custom agents
4. **Monolithic server**: 869-line `server.rs` mixes tool definitions, HTTP handlers, recipe engine, CLI helpers
5. **No type safety across boundary**: Rust schemas → JSON → TypeScript manually synced
6. **No recipe validation**: Invalid recipes fail at runtime with cryptic errors

### 2.3 Opportunity
A **Rust core library** (`herdr-core`) with **WASM bindings** (`herdr-wasm`) enables:
- Any frontend (React, Leptos, TUI, VS Code, Raycast, custom agents) consumes the same toolkit
- Type-safe tool schemas generated from Rust → WASM → TypeScript via `tsify`/`serde_wasm_bindgen`
- Recipe engine runs identically in CLI, HTTP server, WASM (browser), and MCP stdio
- Visual builder (n8n-like) becomes a *skill* consuming the WASM pkg — portable across agents
- Single binary deployment (`herdr-cli`) embeds WASM pkg + serves TUI

---

## 3. Goals & Non-Goals

### 3.1 Goals (v1 scope)
- **Extract `herdr-core`**: Pure Rust library (~300 lines) with zero I/O dependencies — all tools, recipe engine, variable resolver, CLI executor trait
- **Build `herdr-wasm`**: `wasm-bindgen` wrapper exporting `ToolRegistry`, `RecipeEngine`, `VariableStore` — publishes to npm as `herdr-wasm`
- **Create `herdr-cli`**: MCP stdio server + embeds `herdr-wasm/pkg` + serves `herdr-tui` — single binary deployment
- **Create `herdr-server`**: Axum HTTP bridge + serves WASM pkg + static frontend — production deployment target
- **Create `herdr-tui`**: ratatui-based terminal UI consuming `herdr-core` + `herdr-wasm` — native platform feature
- **Publish `herdr-wasm` SKILL.md**: Agent skill format (ponytail/bug-reaper style) for cross-agent portability
- **Type-safe schemas**: Rust `schemars` → `tsify` → TypeScript definitions auto-generated in build

### 3.2 Non-Goals (explicitly out of scope for v1)
- **GraphQL API**: MCP + REST + WASM covers all integration needs
- **Multi-user/auth**: herdr is single-user local daemon — no auth model needed
- **Cloud sync**: Local-only tool; session state lives in herdr daemon
- **Plugin system for tools**: 21 tools are fixed; extensibility via recipes + WASM consumers
- **Real-time collaboration**: Not a multi-user product
- **Mobile app**: Terminal multiplexer doesn't map to mobile

---

## 4. Target Users & Personas

### Persona: Terminal Power User — "Alex"
- **Context**: Senior engineer, lives in tmux/herdr, runs 5-10 agents simultaneously across workspaces
- **Technical level**: Expert — writes shell scripts, knows Rust/TypeScript
- **Primary job to be done**: Automate repetitive herdr workflows (split panes, start agents, wait for output, chain commands)
- **Key frustration today**: Writing fragile bash scripts with `jq` parsing; no visual debugging; can't share recipes with team
- **Success looks like**: "I describe a workflow in 30 seconds, get a visual canvas, run it, and it works every time"

### Persona: Agent Builder — "Sam"
- **Context**: Builds AI agents that need to control terminal sessions (Claude Code, Cursor, custom agents)
- **Technical level**: Expert — integrates MCP servers, writes WASM bindings
- **Primary job to be done**: Give agents reliable, typed access to herdr panes/agents without shell injection
- **Key frustration today**: MCP server is monolithic; can't embed in agent runtime; no type-safe tool definitions
- **Success looks like**: `npm install herdr-wasm` → `const engine = new RecipeEngine()` → done

### Persona: Team Lead — "Jordan"
- **Context**: Manages team using herdr for dev environments; wants standardized, shareable automation
- **Technical level**: Moderate — reads recipes, doesn't write Rust
- **Primary job to be done**: Distribute "golden path" recipes for onboarding, incident response, deployments
- **Key frustration today**: Recipes are raw JSON; no version control friendly format; no visual review
- **Success looks like**: Team shares `.herdr/recipes/*.json` in git; visual builder shows diff; CI validates recipes