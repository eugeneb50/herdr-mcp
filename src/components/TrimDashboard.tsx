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

const fmtBytes = (n: number): string => {
  if (n >= 1_048_576) return `${(n / 1_048_576).toFixed(1)}M`;
  if (n >= 1024) return `${(n / 1024).toFixed(1)}K`;
  return `${n}`;
};

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

  return (
    <div className="w-full px-6 py-12">
      <div className="mb-8">
        <div className="text-xs font-mono text-emerald-400 uppercase tracking-wider mb-3">
          Trim Dashboard
        </div>
        <h1 className="text-3xl md:text-4xl font-bold text-neutral-50 tracking-tight mb-4">
          Message-trim savings
        </h1>
        <p className="text-neutral-400 leading-relaxed max-w-2xl">
          Live aggregate of bytes saved by the caveman + PFC1 pipeline across all
          workspaces. Polls <code className="font-mono text-emerald-300">/api/trim/status</code>{" "}
          every 5s.
        </p>
      </div>

      {error && (
        <div className="rounded-lg border border-amber-700/50 bg-amber-950/30 px-4 py-3 text-amber-300 text-sm">
          Could not reach the trim API: {error}
        </div>
      )}

      {data && (
        <div className="grid gap-6">
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
              <p className="text-neutral-500 text-sm">No trimmed panes yet.</p>
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
