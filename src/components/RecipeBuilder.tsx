import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  DndContext,
  closestCenter,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  sortableKeyboardCoordinates,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { RecipeStepCard } from "./RecipeStep";
import { ResponseViewer } from "./ResponseViewer";
import { VariablePanel } from "./VariablePanel";
import { useVariables } from "./VariableStore";
import type { RecipeDef, RecipeStepDef } from "../recipes";
import { saveRecipe, updateRecipe } from "../recipes/recipeDb";
import type { SavedRecipe } from "../recipes/recipeDb";

type ToolInfo = {
  name: string;
  description: string | null;
  inputSchema: Record<string, unknown>;
};

type Props = {
  initialRecipe: RecipeDef | null;
  onSaved?: () => void;
};

const TOOL_CATEGORIES: Record<string, { label: string; match: string[] }> = {
  session: { label: "Session", match: ["status", "list_"] },
  reads: { label: "Reads", match: ["read_pane", "read_agent"] },
  waits: { label: "Waits", match: ["wait_"] },
  commands: { label: "Commands", match: ["run_", "send_"] },
  agents: { label: "Agents", match: ["get_agent", "start_agent", "stop_agent"] },
  panes: { label: "Pane Ops", match: ["get_pane", "split_pane", "close_pane", "kill_pane", "focus_pane", "resize_pane"] },
  workspace: { label: "Workspace/Tab", match: ["create_", "close_workspace", "close_tab"] },
};

function categorize(tools: ToolInfo[]): { label: string; tools: ToolInfo[] }[] {
  const uncategorized: ToolInfo[] = [];
  const assigned = new Set<string>();
  const groups: { label: string; tools: ToolInfo[] }[] = [];

  for (const [, config] of Object.entries(TOOL_CATEGORIES)) {
    const matched = tools.filter((t) => {
      const inGroup = config.match.some((m) => t.name.startsWith(m) || t.name === m);
      if (inGroup) assigned.add(t.name);
      return inGroup;
    });
    if (matched.length > 0) {
      matched.sort((a, b) => a.name.localeCompare(b.name));
      groups.push({ label: config.label, tools: matched });
    }
  }

  const remaining = tools.filter((t) => !assigned.has(t.name));
  if (remaining.length > 0) {
    remaining.sort((a, b) => a.name.localeCompare(b.name));
    groups.push({ label: "Other", tools: remaining });
  }

  return groups;
}

let stepCounter = 0;
function freshId(): string {
  stepCounter += 1;
  return `step${stepCounter}`;
}

function substituteParams(
  steps: RecipeStepDef[],
  substitute: (t: string) => string,
): RecipeStepDef[] {
  return steps.map((s) => {
    const params: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(s.params)) {
      if (typeof v === "string") {
        params[k] = substitute(v);
      } else if (typeof v === "object" && v !== null) {
        params[k] = JSON.parse(substitute(JSON.stringify(v)));
      } else {
        params[k] = v;
      }
    }
    return { ...s, params };
  });
}

export function RecipeBuilder({ initialRecipe, onSaved }: Props) {
  const [tools, setTools] = useState<ToolInfo[]>([]);
  const [name, setName] = useState(initialRecipe?.name ?? "");
  const [steps, setSteps] = useState<RecipeStepDef[]>(initialRecipe?.steps ?? []);
  const [results, setResults] = useState<Record<string, unknown> | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showVars, setShowVars] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [savedId, setSavedId] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const { substitute, extractFromResponse, varNames } = useVariables();
  const pickerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    (async () => {
      try {
        const res = await fetch("/api/tools");
        const data = await res.json();
        setTools((data.tools ?? []) as ToolInfo[]);
      } catch { /* ignore */ }
    })();
  }, []);

  // Close picker on outside click
  useEffect(() => {
    if (!pickerOpen) return;
    const handle = (e: MouseEvent) => {
      if (pickerRef.current && !pickerRef.current.contains(e.target as Node)) {
        setPickerOpen(false);
      }
    };
    document.addEventListener("mousedown", handle);
    return () => document.removeEventListener("mousedown", handle);
  }, [pickerOpen]);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const addStep = useCallback((tool: string) => {
    setSteps((prev) => [...prev, { id: freshId(), tool, params: {} }]);
    setPickerOpen(false);
  }, []);

  const updateStep = useCallback((id: string, step: Partial<RecipeStepDef>) => {
    setSteps((prev) => prev.map((s) => (s.id === id ? { ...s, ...step } : s)));
  }, []);

  const removeStep = useCallback((id: string) => {
    setSteps((prev) => prev.filter((s) => s.id !== id));
  }, []);

  const handleDragEnd = useCallback((event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    setSteps((prev) => {
      const oldIndex = prev.findIndex((s) => s.id === active.id);
      const newIndex = prev.findIndex((s) => s.id === over.id);
      if (oldIndex === -1 || newIndex === -1) return prev;
      const next = [...prev];
      const [removed] = next.splice(oldIndex, 1);
      next.splice(newIndex, 0, removed);
      return next;
    });
  }, []);

  const stepIds = useMemo(() => steps.map((s) => s.id), [steps]);
  const toolNames = useMemo(() => tools.map((t) => t.name), [tools]);

  const toolMap = useMemo(() => {
    const m = new Map<string, ToolInfo>();
    for (const t of tools) m.set(t.name, t);
    return m;
  }, [tools]);

  const run = useCallback(async () => {
    setLoading(true);
    setError(null);
    setResults(null);
    try {
      const resolved = substituteParams(steps, substitute);
      const res = await fetch("/api/recipe", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name: name || undefined, steps: resolved }),
      });
      const text = await res.text();
      let data: unknown;
      try { data = JSON.parse(text); } catch { data = { raw: text }; }
      if (!res.ok) {
        setError((data as Record<string, unknown>)?.error as string ?? `HTTP ${res.status}`);
      } else {
        setResults(data);
        const recipeResults = ((data as Record<string, unknown>).results ?? {}) as Record<string, unknown>;
        for (const [stepId, result] of Object.entries(recipeResults)) {
          const step = steps.find((s) => s.id === stepId);
          if (step) {
            extractFromResponse(step.tool, result);
          }
        }
      }
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : "Request failed");
    } finally {
      setLoading(false);
    }
  }, [name, steps, substitute, extractFromResponse]);

  const exportRecipe = useCallback(() => {
    const blob = new Blob([JSON.stringify({ name, steps }, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `${name || "recipe"}.json`;
    a.click();
    URL.revokeObjectURL(url);
  }, [name, steps]);

  // Track savedId when loading a saved recipe
  useEffect(() => {
    if (initialRecipe && "id" in initialRecipe) {
      setSavedId((initialRecipe as SavedRecipe).id);
    } else {
      setSavedId(null);
    }
  }, [initialRecipe]);

  const handleSave = useCallback(async () => {
    if (!name.trim()) {
      setSaveError("Recipe name is required");
      return;
    }
    if (steps.length === 0) {
      setSaveError("Add at least one step before saving");
      return;
    }
    setSaveError(null);
    try {
      if (savedId) {
        await updateRecipe(savedId, { name: name.trim(), steps });
      } else {
        const id = await saveRecipe({
          name: name.trim(),
          description: `${steps.length} step(s)`,
          steps,
        });
        setSavedId(id);
      }
      onSaved?.();
    } catch (e) {
      setSaveError(e instanceof Error ? e.message : "Failed to save recipe");
    }
  }, [name, steps, savedId, onSaved]);

  const importRecipe = useCallback(() => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".json";
    input.onchange = async (e) => {
      const file = (e.target as HTMLInputElement).files?.[0];
      if (!file) return;
      const text = await file.text();
      try {
        const recipe = JSON.parse(text);
        setName(recipe.name ?? "");
        setSteps(recipe.steps ?? []);
      } catch {
        setError("Invalid recipe JSON");
      }
    };
    input.click();
  }, []);

  const categorizedTools = useMemo(() => {
    if (tools.length === 0) return [];
    const groups = categorize(tools);
    // Put session first, then commands, reads, waits, etc.
    const order = ["Session", "Reads", "Waits", "Commands", "Agents", "Pane Ops", "Workspace/Tab", "Other"];
    groups.sort((a, b) => {
      const ai = order.indexOf(a.label);
      const bi = order.indexOf(b.label);
      return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi);
    });
    return groups;
  }, [tools]);

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-3">
        <div className="flex-1 relative">
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Recipe name..."
            className="w-full px-3 py-2.5 rounded-lg bg-neutral-900 border border-neutral-700 text-neutral-200 text-sm placeholder-neutral-600 focus:outline-none focus:border-emerald-500/50 transition-colors"
          />
          {savedId && (
            <span className="absolute right-2 top-1/2 -translate-y-1/2 px-1.5 py-0.5 text-[10px] font-mono bg-emerald-500/20 text-emerald-300 rounded">
              Saved
            </span>
          )}
        </div>
        <button
          onClick={() => setShowVars(!showVars)}
          className={`px-3 py-2.5 rounded-lg border text-sm font-mono transition-colors ${
            showVars
              ? "bg-emerald-500/10 border-emerald-500/30 text-emerald-300"
              : "border-neutral-700 text-neutral-400 hover:text-neutral-200"
          }`}
        >
          Variables ({varNames.length})
        </button>
      </div>

      <div className="grid lg:grid-cols-[1fr_260px] gap-6">
        <div className="space-y-2">
          <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
            <SortableContext items={stepIds} strategy={verticalListSortingStrategy}>
              <div className="space-y-2">
                {steps.map((step, i) => (
                  <RecipeStepCard
                    key={step.id}
                    step={step}
                    index={i}
                    allStepIds={stepIds}
                    onChange={(s) => updateStep(step.id, s)}
                    onRemove={() => removeStep(step.id)}
                    result={results ? (results as Record<string, unknown>).results?.[step.id as keyof typeof results] : undefined}
                    tools={tools}
                  />
                ))}
              </div>
            </SortableContext>
          </DndContext>

          {steps.length === 0 && (
            <p className="text-center text-neutral-500 text-sm py-8 italic">
              No steps yet. Click "+ Add Step" to begin.
            </p>
          )}
        </div>

        {showVars && (
          <div className="border-l border-neutral-800/60 pl-6">
            <VariablePanel />
          </div>
        )}
      </div>

      <div className="flex gap-2 relative">
        <div ref={pickerRef} className="relative">
          <button
            onClick={() => setPickerOpen(!pickerOpen)}
            className="px-3 py-2 rounded-lg border border-neutral-700 text-neutral-300 text-sm hover:bg-neutral-800 transition-colors flex items-center gap-1.5"
          >
            + Add Step
            <svg className={`w-3.5 h-3.5 transition-transform ${pickerOpen ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2">
              <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
            </svg>
          </button>

          {pickerOpen && (
            <div className="absolute z-50 bottom-full mb-1 left-0 w-64 bg-neutral-900 border border-neutral-700 rounded-lg shadow-xl max-h-80 overflow-y-auto">
              {categorizedTools.length === 0 && (
                <div className="p-4 text-xs text-neutral-500 italic text-center">Loading tools...</div>
              )}
              {categorizedTools.map((group) => (
                <div key={group.label}>
                  <div className="px-3 py-1.5 text-[10px] font-mono uppercase tracking-wider text-neutral-600 bg-neutral-950/50 sticky top-0">
                    {group.label}
                  </div>
                  {group.tools.map((t) => (
                    <button
                      key={t.name}
                      onClick={() => addStep(t.name)}
                      className="w-full text-left px-3 py-2 text-xs font-mono text-neutral-300 hover:bg-emerald-500/10 hover:text-emerald-200 transition-colors flex flex-col gap-0.5"
                    >
                      <span>{t.name}</span>
                      {t.description && (
                        <span className="text-[10px] text-neutral-500 font-sans line-clamp-1">{t.description}</span>
                      )}
                    </button>
                  ))}
                </div>
              ))}
            </div>
          )}
        </div>

        <button
          onClick={importRecipe}
          className="px-3 py-2 rounded-lg border border-neutral-700 text-neutral-300 text-sm hover:bg-neutral-800 transition-colors"
        >
          Import
        </button>
        <button
          onClick={exportRecipe}
          disabled={steps.length === 0}
          className="px-3 py-2 rounded-lg border border-neutral-700 text-neutral-300 text-sm hover:bg-neutral-800 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
        >
          Export
        </button>
        <button
          onClick={handleSave}
          disabled={steps.length === 0 || !name.trim()}
          className={`px-4 py-2 rounded-lg border text-sm font-mono transition-colors ${
            savedId
              ? "border-emerald-500/50 text-emerald-300 hover:bg-emerald-500/10"
              : "border-neutral-700 text-neutral-300 hover:bg-neutral-800"
          } disabled:opacity-40 disabled:cursor-not-allowed`}
        >
          {savedId ? "Update" : "Save"}
        </button>
        <button
          onClick={run}
          disabled={loading || steps.length === 0}
          className="px-4 py-2 rounded-lg bg-emerald-500 text-neutral-950 font-semibold text-sm hover:bg-emerald-400 disabled:opacity-50 disabled:cursor-not-allowed transition-colors ml-auto"
        >
          {loading ? "Running..." : "Run Recipe"}
        </button>
      </div>

      {saveError && (
        <div className="p-3 rounded-lg bg-rose-500/10 border border-rose-500/20 text-xs text-rose-300">
          {saveError}
        </div>
      )}

      {error && (
        <div className="p-3 rounded-lg bg-rose-500/10 border border-rose-500/20 text-xs text-rose-300">
          {error}
        </div>
      )}

      {results && (
        <div>
          <h3 className="text-xs font-mono uppercase tracking-wider text-neutral-500 mb-2">
            Results — {((results as Record<string, unknown>).status as string) ?? "unknown"}
          </h3>
          <ResponseViewer data={results} />
        </div>
      )}
    </div>
  );
}
