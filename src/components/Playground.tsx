import { useState } from "react";
import { ToolRunner } from "./ToolRunner";
import { RecipeBuilder } from "./RecipeBuilder";
import { RecipeLibrary } from "./RecipeLibrary";
import type { RecipeDef } from "../recipes";

type Tab = "runner" | "builder";

export function Playground() {
  const [tab, setTab] = useState<Tab>(() => {
    try {
      const saved = localStorage.getItem("herdr-mcp.playground.tab");
      return saved === "builder" ? "builder" : "runner";
    } catch {
      return "runner";
    }
  });
  const [currentRecipe, setCurrentRecipe] = useState<RecipeDef | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  const switchTab = (next: Tab) => {
    setTab(next);
    try {
      localStorage.setItem("herdr-mcp.playground.tab", next);
    } catch {
      // ignore storage failures
    }
  };

  return (
    <div className="w-full px-6 py-12">
      <div className="mb-8">
        <div className="text-xs font-mono text-emerald-400 uppercase tracking-wider mb-3">Playground</div>
        <h1 className="text-3xl md:text-4xl font-bold text-neutral-50 tracking-tight mb-4">
          Interactive tool console
        </h1>
        <p className="text-neutral-400 leading-relaxed max-w-2xl">
          Run individual tools or chain them into multi-step recipes with variable passing.
          Requires the server running with <code className="font-mono text-emerald-300">--http &lt;port&gt;</code>.
        </p>
      </div>

      <div className="flex gap-1 mb-8 border-b border-neutral-800/60">
        <button
          onClick={() => switchTab("runner")}
          className={`px-4 py-2.5 text-sm font-mono transition-colors border-b-2 -mb-[1px] ${
            tab === "runner"
              ? "border-emerald-400 text-neutral-100"
              : "border-transparent text-neutral-500 hover:text-neutral-300"
          }`}
        >
          Tool Runner
        </button>
        <button
          onClick={() => switchTab("builder")}
          className={`px-4 py-2.5 text-sm font-mono transition-colors border-b-2 -mb-[1px] ${
            tab === "builder"
              ? "border-emerald-400 text-neutral-100"
              : "border-transparent text-neutral-500 hover:text-neutral-300"
          }`}
        >
          Recipe Builder
        </button>
      </div>

      {/* Keep both tabs mounted so last-used values survive tab switches. */}
      <div style={{ display: tab === "runner" ? undefined : "none" }}>
        <ToolRunner />
      </div>
      <div
        className="grid lg:grid-cols-[280px_1fr] gap-6"
        style={{ display: tab === "builder" ? undefined : "none" }}
      >
        <RecipeLibrary onSelect={(r) => setCurrentRecipe(r)} refreshKey={refreshKey} />
        <RecipeBuilder
          key={currentRecipe?.name ?? "new"}
          initialRecipe={currentRecipe}
          onSaved={() => setRefreshKey((k) => k + 1)}
        />
      </div>
    </div>
  );
}
