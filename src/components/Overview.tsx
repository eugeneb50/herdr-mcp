import { useEffect, useState } from "react";

type HealthData = { status: string };
type TrimStatus = {
  workspace_savings_pct?: number;
  net_saved_bytes?: number;
  messages_trimmed?: number;
};
type ToolsData = { tools: unknown[] };
type Agent = { pane_id: string; label: string; agent: string; status: string; cwd: string; focused: boolean };

export function Overview() {
  const [health, setHealth] = useState<HealthData | null>(null);
  const [trim, setTrim] = useState<TrimStatus | null>(null);
  const [tools, setTools] = useState<ToolsData | null>(null);
  const [agents, setAgents] = useState<Agent[]>([]);

  useEffect(() => {
    const load = async () => {
      try {
        const [h, t, tl, a] = await Promise.all([
          fetch("/api/health").then((r) => r.json()) as Promise<HealthData>,
          fetch("/api/trim/status").then((r) => r.json()) as Promise<TrimStatus>,
          fetch("/api/tools").then((r) => r.json()) as Promise<ToolsData>,
          fetch("/api/agents").then((r) => r.json()) as Promise<Agent[]>,
        ]);
        setHealth(h);
        setTrim(t);
        setTools(tl);
        setAgents(Array.isArray(a) ? a : []);
      } catch { /* server may be unavailable */ }
    };
    load();
  }, []);

  return (
    <div className="max-w-6xl mx-auto px-6 py-10 space-y-8">
      <h1 className="text-xl font-mono font-semibold text-neutral-100">Overview</h1>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <Stat label="Status" value={health?.status ?? "..."} />
        <Stat label="Tools" value={tools?.tools?.length?.toString() ?? "..."} />
        <Stat label="Savings" value={trim?.workspace_savings_pct != null ? `${trim.workspace_savings_pct.toFixed(1)}%` : "..."} />
        <Stat label="Messages Trimmed" value={trim?.messages_trimmed?.toString() ?? "..."} />
      </div>

      <div>
        <h2 className="text-sm font-mono uppercase tracking-wider text-neutral-500 mb-3">Tracked Panes</h2>
        {agents.length === 0 && (
          <p className="text-sm text-neutral-500 italic">No panes tracked. Start a herdr session.</p>
        )}
        {agents.length > 0 && (
          <div className="rounded-lg border border-neutral-800 overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-neutral-800 bg-neutral-900/60">
                  <th className="text-left px-3 py-2 text-[11px] font-mono uppercase tracking-wider text-neutral-500">Label</th>
                  <th className="text-left px-3 py-2 text-[11px] font-mono uppercase tracking-wider text-neutral-500">Agent</th>
                  <th className="text-left px-3 py-2 text-[11px] font-mono uppercase tracking-wider text-neutral-500">Status</th>
                  <th className="text-left px-3 py-2 text-[11px] font-mono uppercase tracking-wider text-neutral-500">CWD</th>
                </tr>
              </thead>
              <tbody>
                {agents.map((a) => (
                  <tr key={a.pane_id} className="border-b border-neutral-800/50 hover:bg-neutral-800/30 transition-colors">
                    <td className="px-3 py-2 font-mono text-neutral-300">
                      {a.focused && <span className="text-emerald-400 mr-1.5">&#9679;</span>}
                      {a.label || a.pane_id.slice(0, 8)}
                    </td>
                    <td className="px-3 py-2 text-neutral-400">{a.agent || "-"}</td>
                    <td className="px-3 py-2">
                      <StatusBadge status={a.status} />
                    </td>
                    <td className="px-3 py-2 text-neutral-500 font-mono text-xs truncate max-w-[200px]">{a.cwd || "-"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-neutral-800 bg-neutral-900/40 p-4">
      <div className="text-[11px] font-mono uppercase tracking-wider text-neutral-500 mb-1">{label}</div>
      <div className="text-lg font-mono font-semibold text-neutral-200">{value}</div>
    </div>
  );
}

function StatusBadge({ status }: { status: string }) {
  const color =
    status === "working" ? "text-emerald-400" :
    status === "done" ? "text-cyan-400" :
    "text-neutral-500";
  return <span className={`text-xs font-mono ${color}`}>{status || "unknown"}</span>;
}
