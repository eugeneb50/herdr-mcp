import { useCallback, useEffect, useMemo, useState } from "react";
import { DynamicForm } from "./DynamicForm";
import { ResponseViewer } from "./ResponseViewer";
import { VariablePanel } from "./VariablePanel";
import { useVariables } from "./VariableStore";

type ToolInfo = {
  name: string;
  description: string | null;
  inputSchema: Record<string, unknown>;
};

export function ToolRunner() {
  const [tools, setTools] = useState<ToolInfo[]>([]);
  const [selected, setSelected] = useState<string>("");
  const [params, setParams] = useState<Record<string, unknown>>({});
  const [response, setResponse] = useState<unknown>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [showVars, setShowVars] = useState(false);
  const { substitute, extractFromResponse, varNames } = useVariables();

  useEffect(() => {
    (async () => {
      try {
        const res = await fetch("/api/tools");
        const data = await res.json();
        setTools(data.tools ?? []);
        if (data.tools?.length > 0) setSelected(data.tools[0].name);
      } catch {
        setError("Cannot connect to server. Start with --http <port>.");
      }
    })();
  }, []);

  const currentTool = tools.find((t) => t.name === selected);
  const schema = currentTool?.inputSchema ?? {};
  const properties = (schema as Record<string, unknown>).properties as Record<string, unknown> | undefined;

  useEffect(() => {
    setParams({});
    setResponse(null);
    setError(null);
  }, [selected]);

  const run = useCallback(async () => {
    if (!selected) return;
    setLoading(true);
    setError(null);
    setResponse(null);
    try {
      // Substitute variables in params
      const resolved: Record<string, unknown> = {};
      for (const [k, v] of Object.entries(params)) {
        if (typeof v === "string") {
          resolved[k] = substitute(v);
        } else {
          resolved[k] = v;
        }
      }

      const res = await fetch(`/api/tools/${selected}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(resolved),
      });
      const data = await res.json();
      if (!res.ok) {
        setError(data.error ?? `HTTP ${res.status}`);
      } else {
        setResponse(data);
        extractFromResponse(selected, data);
      }
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : "Request failed");
    } finally {
      setLoading(false);
    }
  }, [selected, params, substitute, extractFromResponse]);

  const previewParams = useMemo(() => {
    const resolved: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(params)) {
      if (typeof v === "string") {
        resolved[k] = substitute(v);
      } else {
        resolved[k] = v;
      }
    }
    return resolved;
  }, [params, substitute]);

  return (
    <div className="grid lg:grid-cols-[1fr_260px] gap-6">
      <div className="grid lg:grid-cols-[1fr_1fr] gap-6">
        <div className="space-y-4">
          <div>
            <label className="block text-xs font-mono uppercase tracking-wider text-neutral-500 mb-1.5">Tool</label>
            <div className="flex items-center gap-3">
              <div className="flex-1">
                <select
                  value={selected}
                  onChange={(e) => setSelected(e.target.value)}
                  className="w-full px-3 py-2.5 rounded-lg bg-neutral-900 border border-neutral-700 text-neutral-200 text-sm focus:outline-none focus:border-emerald-500/50 transition-colors"
                >
                  {tools.map((t) => (
                    <option key={t.name} value={t.name}>{t.name}</option>
                  ))}
                </select>
                {currentTool?.description && (
                  <p className="text-xs text-neutral-400 mt-1.5">{currentTool.description}</p>
                )}
              </div>
              <button
                onClick={() => setShowVars(!showVars)}
                className={`self-end px-3 py-2.5 rounded-lg border text-sm font-mono transition-colors flex-shrink-0 ${
                  showVars
                    ? "bg-emerald-500/10 border-emerald-500/30 text-emerald-300"
                    : "border-neutral-700 text-neutral-400 hover:text-neutral-200"
                }`}
              >
                Vars ({varNames.length})
              </button>
            </div>
          </div>

          {properties && Object.keys(properties).length > 0 && (
            <DynamicForm
              properties={properties}
              required={((schema as Record<string, unknown>).required as string[]) ?? []}
              values={params}
              onChange={setParams}
              variableNames={varNames}
              substitute={substitute}
            />
          )}

          {properties && Object.keys(properties).length === 0 && (
            <p className="text-xs text-neutral-500 italic">No parameters required</p>
          )}

          {Object.keys(params).length > 0 && (
            <details className="text-xs text-neutral-500">
              <summary className="cursor-pointer hover:text-neutral-300">Resolved params</summary>
              <pre className="mt-1 p-2 rounded bg-neutral-900 font-mono text-[11px] text-neutral-400 whitespace-pre-wrap break-all">
                {JSON.stringify(previewParams, null, 2)}
              </pre>
            </details>
          )}

          <button
            onClick={run}
            disabled={loading}
            className="w-full px-4 py-2.5 rounded-lg bg-emerald-500 text-neutral-950 font-semibold hover:bg-emerald-400 disabled:opacity-50 disabled:cursor-not-allowed transition-colors text-sm"
          >
            {loading ? "Running..." : "Execute"}
          </button>

          {error && (
            <div className="p-3 rounded-lg bg-rose-500/10 border border-rose-500/20 text-xs text-rose-300">
              {error}
            </div>
          )}
        </div>

        <div>
          {response && <ResponseViewer data={response} />}
          {!response && !loading && (
            <div className="h-full flex items-center justify-center text-neutral-600 text-sm">
              Run a tool to see the response
            </div>
          )}
        </div>
      </div>

      {showVars && (
        <div className="border-l border-neutral-800/60 pl-6">
          <VariablePanel />
        </div>
      )}
    </div>
  );
}
