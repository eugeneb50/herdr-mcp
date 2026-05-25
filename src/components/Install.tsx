import { useState } from "react";

type Client = "claude" | "cursor" | "continue";

const CONFIGS: Record<Client, { label: string; path: string; snippet: string }> = {
  claude: {
    label: "Claude Desktop",
    path: "~/.config/Claude/claude_desktop_config.json",
    snippet: `{
  "mcpServers": {
    "herdr-mcp": {
      "command": "/path/to/herdr-mcp"
    }
  }
}`,
  },
  cursor: {
    label: "Cursor",
    path: ".cursor/mcp.json (project) or ~/.cursor/mcp.json (global)",
    snippet: `{
  "mcpServers": {
    "herdr-mcp": {
      "command": "/path/to/herdr-mcp"
    }
  }
}`,
  },
  continue: {
    label: "Continue",
    path: "~/.continue/config.json",
    snippet: `{
  "mcpServers": {
    "herdr-mcp": {
      "name": "herdr-mcp",
      "command": "/path/to/herdr-mcp"
    }
  }
}`,
  },
};

export function Install() {
  const [client, setClient] = useState<Client>("claude");
  const config = CONFIGS[client];

  return (
    <section id="install" className="relative max-w-6xl mx-auto px-6 py-20 border-t border-neutral-800/60">
      <div className="mb-12 max-w-2xl">
        <div className="text-xs font-mono text-emerald-400 uppercase tracking-wider mb-3">Install</div>
        <h2 className="text-3xl md:text-4xl font-bold text-neutral-50 tracking-tight mb-4">
          Three commands. Then you're in.
        </h2>
        <p className="text-neutral-400 leading-relaxed">
          Build the binary, drop a line into your MCP client config, and reload.
          Prerequisites: <code className="font-mono text-emerald-300">rustc</code> 1.75+ and the{" "}
          <a
            href="https://herdr.dev"
            target="_blank"
            rel="noopener"
            className="underline decoration-neutral-600 hover:decoration-emerald-400 transition-colors"
          >
            herdr CLI
          </a>{" "}
          installed.
        </p>
      </div>

      <div className="grid lg:grid-cols-2 gap-5">
        {/* Build step */}
        <div className="rounded-xl border border-neutral-800 bg-neutral-900/40 overflow-hidden">
          <div className="flex items-center justify-between px-5 py-3 border-b border-neutral-800 bg-neutral-950/60">
            <div className="flex items-center gap-2 text-xs font-mono text-neutral-400">
              <span className="w-2 h-2 rounded-full bg-emerald-400" />
              1 · build
            </div>
            <span className="text-[11px] font-mono text-neutral-600">release mode</span>
          </div>
          <pre className="p-5 font-mono text-[13px] text-neutral-300 leading-relaxed overflow-x-auto">
{`$ git clone https://github.com/<you>/herdr-mcp
$ cd herdr-mcp
$ cargo build --release

  Compiling rmcp v1.7.0
  Compiling herdr-mcp v0.1.0
  Finished \`release\` profile

$ ./target/release/herdr-mcp --help`}
          </pre>
        </div>

        {/* Config step */}
        <div className="rounded-xl border border-neutral-800 bg-neutral-900/40 overflow-hidden">
          <div className="flex items-center justify-between px-5 py-3 border-b border-neutral-800 bg-neutral-950/60">
            <div className="flex items-center gap-2 text-xs font-mono text-neutral-400">
              <span className="w-2 h-2 rounded-full bg-emerald-400" />
              2 · configure
            </div>
            <div className="flex gap-1">
              {(Object.keys(CONFIGS) as Client[]).map((key) => (
                <button
                  key={key}
                  onClick={() => setClient(key)}
                  className={`px-2.5 py-1 text-[11px] font-mono rounded transition-colors ${
                    client === key
                      ? "bg-emerald-500/20 text-emerald-300 border border-emerald-500/40"
                      : "text-neutral-500 hover:text-neutral-300 border border-transparent"
                  }`}
                >
                  {CONFIGS[key].label}
                </button>
              ))}
            </div>
          </div>
          <div className="px-5 pt-3 pb-1 text-[11px] font-mono text-neutral-500">{config.path}</div>
          <pre className="px-5 pb-5 font-mono text-[13px] text-neutral-300 leading-relaxed overflow-x-auto">
{config.snippet}
          </pre>
        </div>
      </div>

      <div className="mt-8 p-5 rounded-xl border border-emerald-500/20 bg-emerald-500/5">
        <div className="flex items-start gap-3">
          <svg className="w-5 h-5 text-emerald-400 flex-shrink-0 mt-0.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2">
            <path strokeLinecap="round" strokeLinejoin="round" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
          </svg>
          <div className="text-sm text-neutral-300 leading-relaxed">
            <strong className="text-emerald-300">Tip:</strong> herdr-mcp talks to herdr over the
            same local socket herdr's UI uses. Start herdr first (<code className="font-mono text-emerald-300">herdr</code>),
            then launch your MCP client — they'll share the same session.
          </div>
        </div>
      </div>
    </section>
  );
}
