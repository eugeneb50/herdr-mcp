import { useEffect, useState } from "react";

type PaneStatus = {
  gross_saved_bytes: number;
  net_saved_bytes: number;
  messages_trimmed: number;
};

type TrimStatus = {
  workspace_net_pct: number;
  workspace_savings_pct: number;
  gross_saved_bytes: number;
  net_saved_bytes: number;
  total_input_bytes: number;
  messages_trimmed: number;
  per_pane: Record<string, PaneStatus>;
  active_policies: Record<string, string[]>;
};

type DiagnoseReport = {
  pipeline_roundtrip_ok: boolean;
  pfc1_memory_valid: boolean;
  active_policies: number;
  badge_reachable: boolean;
  sample_roundtrip: {
    input: string;
    compressed: string;
    decompressed: string;
    matches: boolean;
  } | null;
};

const fmtBytes = (n: number): string => {
  if (n >= 1_048_576) return `${(n / 1_048_576).toFixed(1)}M`;
  if (n >= 1024) return `${(n / 1024).toFixed(1)}K`;
  return `${n}`;
};

const POLICY_EXAMPLE =
  'trim_policy_set { target: "agentB", policy: { stages: ["caveman:ultra","pfc1"], direction: "outbound_with_ack" } }';

function Bar({ value, max }: { value: number; max: number }) {
  const pct = max > 0 ? Math.max(2, Math.round((value / max) * 100)) : 0;
  return (
    <div className="h-2 w-full rounded-full bg-neutral-800 overflow-hidden">
      <div
        className="h-full rounded-full bg-gradient-to-r from-emerald-500 to-emerald-300"
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}

export function TrimDashboard() {
  const [data, setData] = useState<TrimStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [diagnoseResult, setDiagnoseResult] = useState<DiagnoseReport | null>(null);
  const [actionLoading, setActionLoading] = useState<string | null>(null);
  const [toast, setToast] = useState<{ message: string; type: "success" | "error" } | null>(null);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const res = await fetch("/api/trim/status");
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const json = (await res.json()) as TrimStatus;
        if (alive) {
          setData(json);
          setError(null);
        }
      } catch (e) {
        if (alive) setError(String(e));
      }
    };
    load();
    const id = setInterval(load, 5000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, []);

  const runAction = async (action: "diagnose" | "summary" | "open") => {
    setActionLoading(action);
    try {
      const endpoints = {
        diagnose: "/api/trim/diagnose",
        summary: "/api/trim/summary",
        open: "/api/trim/dashboard/open",
      };
      const res = await fetch(endpoints[action], {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const json = await res.json();
      if (action === "diagnose") setDiagnoseResult(json as DiagnoseReport);
      if (action === "summary") setToast({ message: "Summary notification sent", type: "success" });
      if (action === "open") setToast({ message: `Opened pane ${(json as { pane_id: string }).pane_id}`, type: "success" });
    } catch (e) {
      setToast({ message: String(e), type: "error" });
    } finally {
      setActionLoading(null);
    }
  };

  return (
    <div className="w-full px-6 py-12">
      <div className="mb-8">
        <div className="flex items-center gap-2 text-xs font-mono text-emerald-400 uppercase tracking-wider mb-3">
          <span className={`w-2 h-2 rounded-full ${data ? "bg-emerald-400 animate-pulse" : "bg-neutral-600"}`} />
          Trim Dashboard
        </div>
        <h1 className="text-3xl md:text-4xl font-bold text-neutral-50 tracking-tight mb-4">
          Message-trim savings
        </h1>
        <p className="text-neutral-400 leading-relaxed max-w-2xl flex items-center gap-4">
          Live aggregate of bytes saved by the caveman + PFC1 pipeline across all
          workspaces. Polls <code className="font-mono text-emerald-300">/api/trim/status</code>{" "}
          every 5s.
          {data && (
            <span className="text-neutral-600 text-sm">
              Last updated: {new Date().toLocaleTimeString()}
            </span>
          )}
        </p>
      </div>

      {toast && (
        <div className={`mb-4 rounded-lg border px-4 py-3 text-sm ${
          toast.type === "success"
            ? "border-emerald-700/50 bg-emerald-950/30 text-emerald-300"
            : "border-amber-700/50 bg-amber-950/30 text-amber-300"
        }`}>
          {toast.message}
        </div>
      )}

      {error && (
        <div className="rounded-lg border border-amber-700/50 bg-amber-950/30 px-4 py-3 text-amber-300 text-sm flex items-center justify-between">
          <span>Could not reach the trim API: {error}</span>
            <button
              type="button"
              onClick={() => {
                setError(null);
              const res = fetch("/api/trim/status");
              res
                .then((r) => {
                  if (!r.ok) throw new Error(`HTTP ${r.status}`);
                  return r.json();
                })
                .then((json) => {
                  setData(json as TrimStatus);
                  setError(null);
                })
                .catch((e) => setError(String(e)));
            }}
            className="px-3 py-1 text-xs font-medium rounded border border-amber-600 hover:bg-amber-900/30 transition-colors"
          >
            Retry
          </button>
        </div>
      )}

      {!data && !error && (
        <div className="space-y-6" aria-busy="true">
          <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
            {[0, 1, 2, 3].map((i) => (
              <div key={i} className="rounded-xl border border-neutral-800/60 bg-neutral-900/40 p-4 animate-pulse">
                <div className="h-3 w-1/3 bg-neutral-800 rounded mb-2" />
                <div className="h-8 w-3/4 bg-neutral-800 rounded" />
              </div>
            ))}
          </div>
          <div className="rounded-xl border border-neutral-800/60 bg-neutral-900/40 p-5 animate-pulse">
            <div className="h-4 w-1/4 bg-neutral-800 rounded mb-4" />
            <div className="space-y-3">
              {[0, 1, 2].map((i) => (
                <div key={i} className="h-8 bg-neutral-800 rounded" />
              ))}
            </div>
          </div>
        </div>
      )}

      {data && (
        <div className="grid gap-6">
          <div className="flex flex-wrap gap-3">
            <ActionButton label="Run Diagnostics" loading={actionLoading === "diagnose"} onClick={() => runAction("diagnose")} />
            <ActionButton label="Send Summary" loading={actionLoading === "summary"} onClick={() => runAction("summary")} />
            <ActionButton label="Open Dashboard" loading={actionLoading === "open"} onClick={() => runAction("open")} />
          </div>

          {diagnoseResult && (
            <section className="rounded-xl border border-neutral-800/60 bg-neutral-900/40 p-5">
              <div className="flex items-center justify-between mb-3">
                <h2 className="text-sm font-mono text-neutral-300 uppercase tracking-wider">
                  Diagnostics Result
                </h2>
                <button
                  type="button"
                  onClick={() => setDiagnoseResult(null)}
                  className="text-neutral-500 hover:text-neutral-200 text-sm"
                >
                  ×
                </button>
              </div>
              <div className="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
                <Stat label="Round-trip OK" value={diagnoseResult.pipeline_roundtrip_ok ? "✅" : "❌"} />
                <Stat label="PFC1 Memory" value={diagnoseResult.pfc1_memory_valid ? "✅" : "❌"} />
                <Stat label="Active Policies" value={String(diagnoseResult.active_policies)} />
                <Stat label="Badge Reachable" value={diagnoseResult.badge_reachable ? "✅" : "❌"} />
              </div>
              {diagnoseResult.sample_roundtrip && (() => {
                const s = diagnoseResult.sample_roundtrip!;
                const sampleText = `Input:       ${s.input}\nCompressed:  ${s.compressed}\nDecompressed:${s.decompressed}\nMatches:     ${s.matches ? "Yes" : "No"}`;
                return (
                  <details className="mt-4 text-xs font-mono text-neutral-400">
                    <summary className="cursor-pointer mb-2">Sample Round-trip</summary>
                    <pre className="bg-neutral-950 p-3 rounded overflow-x-auto whitespace-pre-wrap">
{sampleText}
                    </pre>
                  </details>
                );
              })()}
            </section>
          )}

          <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
            <Stat
              label="Savings %"
              value={data.workspace_savings_pct > 0 ? `-${data.workspace_savings_pct.toFixed(1)}%` : "—"}
              accent
            />
            <Stat label="Net saved" value={`${fmtBytes(data.net_saved_bytes)}B`} />
            <Stat label="Gross saved" value={`${fmtBytes(data.gross_saved_bytes)}B`} />
            <Stat label="Messages" value={String(data.messages_trimmed)} />
          </div>

          <section className="rounded-xl border border-neutral-800/60 bg-neutral-900/40 p-5">
            <h2 className="text-sm font-mono text-neutral-300 uppercase tracking-wider mb-4">
              Per-pane
            </h2>
            {Object.keys(data.per_pane).length === 0 ? (
              <div className="text-center">
                <div className="text-4xl mb-3">📊</div>
                <p className="text-neutral-400 mb-4">No trimmed panes yet.</p>
                <p className="text-neutral-500 text-sm mb-4">
                  Set a trim policy on a pane to start tracking savings:
                </p>
                <code className="font-mono text-emerald-300 bg-neutral-950 px-3 py-1.5 rounded block max-w-xs mx-auto break-all">
                  {POLICY_EXAMPLE}
                </code>
              </div>
            ) : (
              <div className="space-y-3">
                {Object.entries(data.per_pane)
                  .sort((a, b) => b[1].net_saved_bytes - a[1].net_saved_bytes)
                  .map(([pane, p]) => (
                    <div key={pane} className="grid grid-cols-[160px_1fr_80px] items-center gap-3">
                      <span className="font-mono text-xs text-neutral-400 truncate">{pane}</span>
                      <Bar value={p.net_saved_bytes} max={Math.max(1, ...Object.values(data.per_pane).map((x) => x.net_saved_bytes))} />
                      <span className="text-right font-mono text-xs text-emerald-300">
                        {fmtBytes(p.net_saved_bytes)}B
                      </span>
                    </div>
                  ))}
              </div>
            )}
          </section>

          <section className="rounded-xl border border-neutral-800/60 bg-neutral-900/40 p-5">
            <h2 className="text-sm font-mono text-neutral-300 uppercase tracking-wider mb-4">
              Active policies
            </h2>
            {Object.keys(data.active_policies).length === 0 ? (
              <p className="text-neutral-500 text-sm">No active trim policies.</p>
            ) : (
              <div className="space-y-2">
                {Object.entries(data.active_policies).map(([pane, stages]) => (
                  <div key={pane} className="flex items-center gap-3 flex-wrap">
                    <span className="font-mono text-xs text-neutral-400">{pane}</span>
                    <div className="flex gap-1.5">
                      {stages.map((s) => (
                        <span
                          key={s}
                          className="text-[11px] font-mono px-2 py-0.5 rounded bg-neutral-800 text-emerald-300"
                        >
                          {s}
                        </span>
                      ))}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </section>
        </div>
      )}
    </div>
  );
}

function Stat({ label, value, accent }: { label: string; value: string; accent?: boolean }) {
  return (
    <div className="rounded-xl border border-neutral-800/60 bg-neutral-900/40 p-4">
      <div className="text-xs text-neutral-500 mb-1">{label}</div>
      <div className={`text-2xl font-bold font-mono ${accent ? "text-emerald-300" : "text-neutral-100"}`}>
        {value}
      </div>
    </div>
  );
}

function ActionButton({ label, loading, onClick }: { label: string; loading: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={loading}
      className="px-3 py-1.5 text-sm font-medium rounded-md border border-neutral-700 bg-neutral-900/60 hover:bg-neutral-800 hover:border-neutral-600 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
    >
      {loading ? "⏳" : "▶"} {label}
    </button>
  );
}
