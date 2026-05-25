import { useMemo, useState } from "react";

export function ResponseViewer({ data }: { data: unknown }) {
  const [tab, setTab] = useState<"formatted" | "raw">("formatted");

  const { content, isError } = useMemo(() => {
    const d = data as Record<string, unknown>;
    return {
      content: d.content as Array<Record<string, unknown>> ?? [],
      isError: !!d.isError,
    };
  }, [data]);

  return (
    <div className="rounded-lg border border-neutral-800 bg-neutral-900/40 overflow-hidden">
      <div className="flex items-center justify-between px-4 py-2.5 border-b border-neutral-800/60">
        <div className="flex items-center gap-2">
          <span className="text-xs font-mono uppercase tracking-wider text-neutral-500">Response</span>
          {isError && (
            <span className="px-1.5 py-0.5 text-[10px] font-mono bg-rose-500/20 text-rose-300 rounded">error</span>
          )}
        </div>
        <div className="flex gap-1">
          <button
            onClick={() => setTab("formatted")}
            className={`px-2 py-1 text-[11px] font-mono rounded ${
              tab === "formatted"
                ? "bg-neutral-800 text-neutral-200"
                : "text-neutral-500 hover:text-neutral-300"
            }`}
          >
            Formatted
          </button>
          <button
            onClick={() => setTab("raw")}
            className={`px-2 py-1 text-[11px] font-mono rounded ${
              tab === "raw"
                ? "bg-neutral-800 text-neutral-200"
                : "text-neutral-500 hover:text-neutral-300"
            }`}
          >
            Raw JSON
          </button>
        </div>
      </div>

      <div className="p-4 max-h-[60vh] overflow-y-auto">
        {tab === "raw" && (
          <pre className="text-xs font-mono text-neutral-300 leading-relaxed whitespace-pre-wrap">
            {JSON.stringify(data, null, 2)}
          </pre>
        )}

        {tab === "formatted" && (
          <div className="space-y-3">
            {content.length === 0 && !isError && (
              <p className="text-xs text-neutral-500 italic">Empty response</p>
            )}
            {content.map((c, i) => {
              const type = c.type as string;
              const text = c.text as string;
              const mimeType = c.mimeType as string | undefined;

              if (type === "text" && mimeType === "application/json") {
                try {
                  const parsed = JSON.parse(text);
                  return (
                    <div key={i}>
                      <div className="text-[11px] font-mono text-neutral-500 mb-1">JSON content</div>
                      <pre className="text-xs font-mono text-neutral-200 bg-neutral-950/60 rounded-md p-3 whitespace-pre-wrap break-all">
                        {JSON.stringify(parsed, null, 2)}
                      </pre>
                    </div>
                  );
                } catch {
                  return (
                    <div key={i} className="text-xs text-neutral-300 font-mono whitespace-pre-wrap">
                      {text}
                    </div>
                  );
                }
              }

              return (
                <div key={i} className="text-xs text-neutral-300 font-mono whitespace-pre-wrap">
                  {text}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
