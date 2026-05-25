const TOOLS = [
  {
    category: "Discovery",
    description: "See what's running and where.",
    icon: (
      <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
        <path strokeLinecap="round" strokeLinejoin="round" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
      </svg>
    ),
    accent: "sky",
    tools: [
      { name: "status", desc: "Get herdr server & client status" },
      { name: "list_workspaces", desc: "List all workspaces" },
      { name: "list_tabs", desc: "List tabs (optionally by workspace)" },
      { name: "list_panes", desc: "List panes (optionally by workspace)" },
      { name: "list_agents", desc: "List all detected agents" },
      { name: "get_pane", desc: "Pane details by id" },
      { name: "get_agent", desc: "Agent details by target" },
    ],
  },
  {
    category: "Lifecycle",
    description: "Create and tear down structure.",
    icon: (
      <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
        <path strokeLinecap="round" strokeLinejoin="round" d="M4 6h16M4 10h16M4 14h7m-7 4h11" />
      </svg>
    ),
    accent: "emerald",
    tools: [
      { name: "create_workspace", desc: "Create a workspace" },
      { name: "create_tab", desc: "Create a tab" },
      { name: "split_pane", desc: "Split a pane right or down" },
      { name: "close_pane", desc: "Close a pane" },
      { name: "start_agent", desc: "Start an agent in a new pane" },
    ],
  },
  {
    category: "Read",
    description: "Inspect pane and agent output.",
    icon: (
      <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
        <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
        <path strokeLinecap="round" strokeLinejoin="round" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
      </svg>
    ),
    accent: "teal",
    tools: [
      { name: "read_pane", desc: "Read pane output (visible / recent / unwrapped)" },
      { name: "read_agent", desc: "Read agent output" },
    ],
  },
  {
    category: "Write",
    description: "Send input to panes and agents.",
    icon: (
      <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
        <path strokeLinecap="round" strokeLinejoin="round" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
      </svg>
    ),
    accent: "amber",
    tools: [
      { name: "send_text", desc: "Send text to a pane (no Enter)" },
      { name: "send_keys", desc: "Send key presses (Enter, Ctrl+c, etc.)" },
      { name: "run_command", desc: "Send text + Enter atomically" },
      { name: "send_agent", desc: "Send text to an agent's stream" },
    ],
  },
  {
    category: "Synchronize",
    description: "The killer feature for multi-agent orchestration.",
    icon: (
      <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
        <path strokeLinecap="round" strokeLinejoin="round" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
      </svg>
    ),
    accent: "rose",
    highlight: true,
    tools: [
      { name: "wait_output", desc: "Block until text appears (regex supported)" },
      { name: "wait_pane_agent_status", desc: "Wait for a pane's agent status" },
      { name: "wait_agent_status", desc: "Wait for an agent by target" },
    ],
  },
];

const ACCENT_STYLES: Record<string, { chip: string; border: string; icon: string; glow: string }> = {
  sky: {
    chip: "bg-sky-500/10 text-sky-300 border-sky-500/30",
    border: "border-sky-500/20",
    icon: "text-sky-400",
    glow: "bg-sky-500/10",
  },
  emerald: {
    chip: "bg-emerald-500/10 text-emerald-300 border-emerald-500/30",
    border: "border-emerald-500/20",
    icon: "text-emerald-400",
    glow: "bg-emerald-500/10",
  },
  teal: {
    chip: "bg-teal-500/10 text-teal-300 border-teal-500/30",
    border: "border-teal-500/20",
    icon: "text-teal-400",
    glow: "bg-teal-500/10",
  },
  amber: {
    chip: "bg-amber-500/10 text-amber-300 border-amber-500/30",
    border: "border-amber-500/20",
    icon: "text-amber-400",
    glow: "bg-amber-500/10",
  },
  rose: {
    chip: "bg-rose-500/10 text-rose-300 border-rose-500/30",
    border: "border-rose-500/30",
    icon: "text-rose-400",
    glow: "bg-rose-500/10",
  },
};

export function Tools() {
  return (
    <section id="tools" className="relative max-w-6xl mx-auto px-6 py-20 border-t border-neutral-800/60">
      <div className="mb-12 max-w-2xl">
        <div className="text-xs font-mono text-emerald-400 uppercase tracking-wider mb-3">Tools</div>
        <h2 className="text-3xl md:text-4xl font-bold text-neutral-50 tracking-tight mb-4">
          21 tools, grouped by intent.
        </h2>
        <p className="text-neutral-400 leading-relaxed">
          The orchestration-relevant subset of herdr, exposed as strongly-typed MCP tools
          with JSON schemas auto-generated from Rust structs.
        </p>
      </div>

      <div className="grid md:grid-cols-2 gap-5">
        {TOOLS.map((group) => {
          const style = ACCENT_STYLES[group.accent];
          return (
            <div
              key={group.category}
              className={`relative rounded-xl border ${style.border} bg-neutral-900/40 p-6 backdrop-blur-sm ${
                group.highlight ? "md:col-span-2 ring-1 ring-rose-500/20" : ""
              }`}
            >
              {group.highlight && (
                <div className="absolute -top-3 right-6 px-2.5 py-0.5 text-[10px] font-mono uppercase tracking-wider bg-rose-500 text-white rounded-full">
                  Orchestration primitive
                </div>
              )}
              <div className="flex items-start gap-4 mb-5">
                <div
                  className={`flex-shrink-0 w-10 h-10 rounded-lg flex items-center justify-center border ${style.chip}`}
                >
                  {group.icon}
                </div>
                <div>
                  <h3 className="text-lg font-semibold text-neutral-100">{group.category}</h3>
                  <p className="text-sm text-neutral-400">{group.description}</p>
                </div>
              </div>
              <div className={`grid ${group.highlight ? "md:grid-cols-3" : ""} gap-2`}>
                {group.tools.map((tool) => (
                  <div
                    key={tool.name}
                    className="group p-3 rounded-md bg-neutral-950/50 border border-neutral-800/60 hover:border-neutral-700 transition-colors"
                  >
                    <div className="flex items-center gap-2 mb-1">
                      <span className={`w-1 h-1 rounded-full ${style.glow.replace("/10", "/60")}`} />
                      <code className="font-mono text-sm text-neutral-100">{tool.name}</code>
                    </div>
                    <p className="text-xs text-neutral-500 leading-relaxed pl-3">{tool.desc}</p>
                  </div>
                ))}
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}
