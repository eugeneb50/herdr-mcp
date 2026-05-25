import { useEffect, useMemo, useState } from "react";

type ToolInfo = {
  name: string;
  description: string | null;
  inputSchema: Record<string, unknown>;
};

const CATEGORIES = [
  { key: "Discovery", accent: "sky", tools: ["status", "list_workspaces", "list_tabs", "list_panes", "list_agents", "get_pane", "get_agent"] },
  { key: "Lifecycle", accent: "emerald", tools: ["create_workspace", "create_tab", "split_pane", "close_pane", "start_agent"] },
  { key: "Read", accent: "teal", tools: ["read_pane", "read_agent"] },
  { key: "Write", accent: "amber", tools: ["send_text", "send_keys", "run_command", "send_agent"] },
  { key: "Synchronize", accent: "rose", tools: ["wait_output", "wait_pane_agent_status", "wait_agent_status"] },
] as const;

const ACCENT_STYLES: Record<string, { chip: string; border: string; icon: string }> = {
  sky: { chip: "bg-sky-500/10 text-sky-300 border-sky-500/30", border: "border-sky-500/20", icon: "text-sky-400" },
  emerald: { chip: "bg-emerald-500/10 text-emerald-300 border-emerald-500/30", border: "border-emerald-500/20", icon: "text-emerald-400" },
  teal: { chip: "bg-teal-500/10 text-teal-300 border-teal-500/30", border: "border-teal-500/20", icon: "text-teal-400" },
  amber: { chip: "bg-amber-500/10 text-amber-300 border-amber-500/30", border: "border-amber-500/20", icon: "text-amber-400" },
  rose: { chip: "bg-rose-500/10 text-rose-300 border-rose-500/30", border: "border-rose-500/30", icon: "text-rose-400" },
};

const CATEGORY_ICONS: Record<string, React.ReactNode> = {
  Discovery: (
    <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
      <path strokeLinecap="round" strokeLinejoin="round" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
    </svg>
  ),
  Lifecycle: (
    <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
      <path strokeLinecap="round" strokeLinejoin="round" d="M4 6h16M4 10h16M4 14h7m-7 4h11" />
    </svg>
  ),
  Read: (
    <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
      <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
    </svg>
  ),
  Write: (
    <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
      <path strokeLinecap="round" strokeLinejoin="round" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
    </svg>
  ),
  Synchronize: (
    <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
      <path strokeLinecap="round" strokeLinejoin="round" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
    </svg>
  ),
};

function toolCategory(name: string): string | undefined {
  for (const cat of CATEGORIES) {
    if (cat.tools.includes(name)) return cat.accent;
  }
  return undefined;
}

export function Documentation() {
  const [tools, setTools] = useState<ToolInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");

  useEffect(() => {
    (async () => {
      try {
        const res = await fetch("/api/tools");
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data = await res.json();
        setTools(data.tools ?? []);
      } catch (e: unknown) {
        setError(e instanceof Error ? e.message : "Failed to load tools");
      }
    })();
  }, []);

  const filtered = useMemo(() => {
    if (!tools) return [];
    if (!search.trim()) return tools;
    const q = search.toLowerCase();
    return tools.filter((t) => t.name.toLowerCase().includes(q));
  }, [tools, search]);

  return (
    <div className="max-w-6xl mx-auto px-6 py-12">
      <div className="mb-10">
        <div className="text-xs font-mono text-emerald-400 uppercase tracking-wider mb-3">Documentation</div>
        <h1 className="text-3xl md:text-4xl font-bold text-neutral-50 tracking-tight mb-4">
          Full tool reference
        </h1>
        <p className="text-neutral-400 leading-relaxed max-w-2xl">
          All 21 herdr-mcp tools with their JSON schemas, parameter details, and example usage.
        </p>
      </div>

      <div className="relative mb-8 max-w-md">
        <svg className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-neutral-500" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2">
          <path strokeLinecap="round" strokeLinejoin="round" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
        </svg>
        <input
          type="text"
          placeholder="Search tools..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          className="w-full pl-10 pr-4 py-2.5 rounded-lg bg-neutral-900 border border-neutral-700 text-neutral-200 text-sm placeholder-neutral-500 focus:outline-none focus:border-emerald-500/50 transition-colors"
        />
      </div>

      {error && (
        <div className="p-5 rounded-xl border border-rose-500/20 bg-rose-500/5 text-sm text-rose-300 mb-8">
          <strong>Could not connect to server.</strong> Start the server with <code className="font-mono">--http &lt;port&gt;</code> to see live tool schemas.
          <div className="mt-2 text-neutral-400">Error: {error}</div>
        </div>
      )}

      {!tools && !error && (
        <div className="text-neutral-500 text-sm animate-pulse">Loading tools...</div>
      )}

      {filtered.length === 0 && tools && (
        <div className="text-neutral-500 text-sm">No tools match "{search}"</div>
      )}

      {tools && filtered.length > 0 && (
        <div className="space-y-12">
          {CATEGORIES.map((cat) => {
            const catTools = filtered.filter((t) => cat.tools.includes(t.name));
            if (catTools.length === 0) return null;
            const style = ACCENT_STYLES[cat.accent];
            return (
              <div key={cat.key}>
                <div className={`flex items-center gap-3 mb-5`}>
                  <div className={`flex-shrink-0 w-9 h-9 rounded-lg flex items-center justify-center border ${style.chip}`}>
                    {CATEGORY_ICONS[cat.key]}
                  </div>
                  <div>
                    <h2 className="text-xl font-semibold text-neutral-100">{cat.key}</h2>
                  </div>
                </div>
                <div className="grid md:grid-cols-2 gap-4">
                  {catTools.map((tool) => (
                    <ToolDocCard key={tool.name} tool={tool} accent={cat.accent} />
                  ))}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function ToolDocCard({ tool, accent }: { tool: ToolInfo; accent: string }) {
  const [expanded, setExpanded] = useState(false);
  const style = ACCENT_STYLES[accent] ?? ACCENT_STYLES.sky;

  const schema = tool.inputSchema ?? {};
  const properties = (schema as Record<string, unknown>).properties as Record<string, unknown> | undefined;
  const required = ((schema as Record<string, unknown>).required as string[]) ?? [];

  return (
    <div
      className={`rounded-lg border ${style.border} bg-neutral-900/40 overflow-hidden transition-all`}
    >
      <button
        onClick={() => setExpanded(!expanded)}
        className="w-full flex items-center justify-between p-4 hover:bg-neutral-800/40 transition-colors text-left"
      >
        <div>
          <code className="font-mono text-sm text-neutral-100">{tool.name}</code>
          {tool.description && (
            <p className="text-xs text-neutral-400 mt-0.5 line-clamp-1">{tool.description}</p>
          )}
        </div>
        <svg
          className={`w-4 h-4 text-neutral-500 transition-transform ${expanded ? "rotate-180" : ""}`}
          fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2"
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {expanded && (
        <div className="border-t border-neutral-800/60 p-4 space-y-4">
          {properties && Object.keys(properties).length > 0 && (
            <div>
              <h4 className="text-xs font-mono uppercase tracking-wider text-neutral-500 mb-2">Parameters</h4>
              <div className="space-y-2">
                {Object.entries(properties).map(([key, value]) => {
                  const prop = value as Record<string, unknown>;
                  const isRequired = required.includes(key);
                  return (
                    <div key={key} className="flex items-start gap-3 text-xs">
                      <code className="font-mono text-emerald-300 flex-shrink-0">{key}</code>
                      <span className="text-neutral-500 font-mono">{(prop.type as string) ?? "any"}</span>
                      {isRequired && <span className="text-rose-400">required</span>}
                      {prop.description && (
                        <span className="text-neutral-400 ml-auto text-right">{prop.description as string}</span>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          <div>
            <h4 className="text-xs font-mono uppercase tracking-wider text-neutral-500 mb-2">JSON Schema</h4>
            <pre className="text-xs font-mono text-neutral-300 bg-neutral-950/60 rounded-md p-3 overflow-x-auto max-h-48 overflow-y-auto">
              {JSON.stringify(schema, null, 2)}
            </pre>
          </div>

          <div>
            <h4 className="text-xs font-mono uppercase tracking-wider text-neutral-500 mb-2">Example Request</h4>
            <pre className="text-xs font-mono text-neutral-300 bg-neutral-950/60 rounded-md p-3 overflow-x-auto">
{`POST /api/tools/${tool.name}
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "${tool.name}",
    "arguments": ${JSON.stringify(exampleArgs(properties), null, 2)}
  }
}`}
            </pre>
          </div>
        </div>
      )}
    </div>
  );
}

function exampleArgs(properties: Record<string, unknown> | undefined): Record<string, string> {
  if (!properties) return {};
  const args: Record<string, string> = {};
  for (const key of Object.keys(properties)) {
    args[key] = `<${key}>`;
  }
  return args;
}
