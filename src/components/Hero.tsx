export function Hero() {
  return (
    <section className="relative max-w-6xl mx-auto px-6 pt-20 pb-24 md:pt-28 md:pb-32">
      <div className="grid md:grid-cols-2 gap-12 items-center">
        {/* Left: copy */}
        <div>
          <div className="inline-flex items-center gap-2 px-3 py-1 rounded-full border border-emerald-500/30 bg-emerald-500/5 text-emerald-300 text-xs font-mono mb-6">
            <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 glow" />
            MCP server · stdio · Rust
          </div>

          <h1 className="text-4xl md:text-5xl font-bold text-neutral-50 tracking-tight leading-[1.1] mb-5">
            Let AI orchestrate{" "}
            <span className="bg-gradient-to-r from-emerald-300 to-teal-400 bg-clip-text text-transparent">
              your whole herd
            </span>
            .
          </h1>

          <p className="text-lg text-neutral-400 leading-relaxed mb-8">
            <code className="font-mono text-emerald-300">herdr-mcp</code> is a thin MCP server
            that exposes{" "}
            <a
              href="https://herdr.dev"
              target="_blank"
              rel="noopener"
              className="underline decoration-neutral-600 hover:decoration-emerald-400 transition-colors"
            >
              herdr
            </a>{" "}
            — the terminal-native agent multiplexer — as tools. Give Claude, Cursor, and
            other MCP clients the ability to spawn agents, read pane output, send commands,
            and wait for state changes across your entire workspace.
          </p>

          <div className="flex flex-wrap gap-3">
            <button
              onClick={() => document.getElementById("install")?.scrollIntoView({ behavior: "smooth" })}
              className="inline-flex items-center gap-2 px-4 py-2.5 rounded-md bg-emerald-500 text-neutral-950 font-semibold hover:bg-emerald-400 transition-colors cursor-pointer"
            >
              Get started
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2.5">
                <path strokeLinecap="round" strokeLinejoin="round" d="M13 7l5 5-5 5M6 12h12" />
              </svg>
            </button>
            <button
              onClick={() => document.getElementById("tools")?.scrollIntoView({ behavior: "smooth" })}
              className="inline-flex items-center gap-2 px-4 py-2.5 rounded-md border border-neutral-700 bg-neutral-900/40 text-neutral-200 hover:border-neutral-600 hover:bg-neutral-900 transition-colors cursor-pointer"
            >
              See the tools
            </button>
          </div>

          <div className="flex flex-wrap gap-x-8 gap-y-3 mt-10 text-sm text-neutral-500">
            <div className="flex items-center gap-2">
              <svg className="w-4 h-4 text-emerald-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2.5">
                <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
              </svg>
              Works with Claude, Cursor, Continue
            </div>
            <div className="flex items-center gap-2">
              <svg className="w-4 h-4 text-emerald-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2.5">
                <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
              </svg>
              21 tools across 5 categories
            </div>
            <div className="flex items-center gap-2">
              <svg className="w-4 h-4 text-emerald-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2.5">
                <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
              </svg>
              Single static binary
            </div>
          </div>
        </div>

        {/* Right: terminal mockup */}
        <div className="relative">
          <div className="absolute -inset-4 bg-gradient-to-br from-emerald-500/10 via-transparent to-teal-500/5 rounded-3xl blur-2xl" />
          <div className="relative rounded-xl border border-neutral-800 bg-neutral-900/80 shadow-2xl shadow-emerald-500/5 overflow-hidden">
            <div className="flex items-center gap-2 px-4 py-3 border-b border-neutral-800 bg-neutral-950/60">
              <div className="flex gap-1.5">
                <div className="w-2.5 h-2.5 rounded-full bg-neutral-700" />
                <div className="w-2.5 h-2.5 rounded-full bg-neutral-700" />
                <div className="w-2.5 h-2.5 rounded-full bg-neutral-700" />
              </div>
              <div className="ml-3 text-xs font-mono text-neutral-500">claude · tool call</div>
            </div>
            <div className="p-5 font-mono text-[13px] leading-relaxed space-y-3">
              <div>
                <span className="text-neutral-500">›</span>{" "}
                <span className="text-neutral-400">calling tool</span>{" "}
                <span className="text-emerald-300">list_panes</span>
              </div>
              <div className="pl-3 text-neutral-400">
                <span className="text-neutral-600">┌─</span> result
              </div>
              <div className="pl-3 text-neutral-400">
                <span className="text-neutral-600">│</span>{" "}
                <span className="text-neutral-500">[</span>
              </div>
              <div className="pl-6 text-neutral-300">
                <span className="text-neutral-600">│</span>{" "}
                <span className="text-neutral-500">{"{"}</span>{" "}
                <span className="text-amber-300">"pane_id"</span>:{" "}
                <span className="text-emerald-300">"1-1"</span>,{" "}
                <span className="text-amber-300">"agent_status"</span>:{" "}
                <span className="text-emerald-300">"working"</span>{" "}
                <span className="text-neutral-500">{"}"}</span>
              </div>
              <div className="pl-6 text-neutral-300">
                <span className="text-neutral-600">│</span>{" "}
                <span className="text-neutral-500">{"{"}</span>{" "}
                <span className="text-amber-300">"pane_id"</span>:{" "}
                <span className="text-emerald-300">"1-2"</span>,{" "}
                <span className="text-amber-300">"agent_status"</span>:{" "}
                <span className="text-rose-300">"blocked"</span>{" "}
                <span className="text-neutral-500">{"}"}</span>
              </div>
              <div className="pl-3 text-neutral-400">
                <span className="text-neutral-600">│</span>{" "}
                <span className="text-neutral-500">]</span>
              </div>
              <div className="pt-2 text-neutral-400">
                <span className="text-neutral-500">›</span>{" "}
                <span className="text-neutral-400">calling tool</span>{" "}
                <span className="text-emerald-300">wait_agent_status</span>
              </div>
              <div className="pl-3 text-neutral-500">
                <span className="text-neutral-600">└─</span> waiting for pane{" "}
                <span className="text-emerald-300">1-2</span> →{" "}
                <span className="text-emerald-300">done</span>
                <span className="cursor-blink ml-0.5 text-emerald-400">▊</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
