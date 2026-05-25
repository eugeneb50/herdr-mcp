import { useEffect, useRef, useState } from "react";
import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import type { RecipeStepDef } from "../recipes";
import { DynamicForm } from "./DynamicForm";
import { useVariables } from "./VariableStore";

type ToolOption = {
  name: string;
  description: string | null;
  inputSchema: Record<string, unknown>;
};

type Props = {
  step: RecipeStepDef;
  index: number;
  allStepIds: string[];
  onChange: (update: Partial<RecipeStepDef>) => void;
  onRemove: () => void;
  result: unknown;
  tools: ToolOption[];
};

export function RecipeStepCard({ step, index, onChange, onRemove, tools }: Props) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: step.id });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.4 : 1,
  };

  const schema = tools.find((t) => t.name === step.tool)?.inputSchema ?? null;
  const { variableNames: varNames, substitute } = useVariables();

  const schemaProps = (schema?.properties as Record<string, unknown>) ?? null;
  const schemaRequired = (schema?.required as string[]) ?? [];

  const handleToolChange = (newTool: string) => {
    // Preserve params whose keys exist in both old and new tool schemas
    const oldTool = tools.find((t) => t.name === step.tool);
    const newToolInfo = tools.find((t) => t.name === newTool);
    if (oldTool && newToolInfo) {
      const oldProps = (oldTool.inputSchema?.properties as Record<string, unknown>) ?? {};
      const newProps = (newToolInfo.inputSchema?.properties as Record<string, unknown>) ?? {};
      const preserved: Record<string, unknown> = {};
      for (const [k, v] of Object.entries(step.params)) {
        if (k in newProps) preserved[k] = v;
      }
      // Set default values from schema for new required fields
      for (const key of (newToolInfo.inputSchema?.required as string[]) ?? []) {
        if (!(key in preserved)) {
          preserved[key] = "";
        }
      }
      onChange({ tool: newTool, params: preserved });
    } else {
      onChange({ tool: newTool, params: {} });
    }
  };

  return (
    <div
      ref={setNodeRef}
      style={style}
      className="rounded-lg border border-neutral-800 bg-neutral-900/60 p-4"
    >
      <div className="flex items-center gap-3 mb-3">
        <button
          {...attributes}
          {...listeners}
          className="cursor-grab text-neutral-600 hover:text-neutral-400 transition-colors"
          title="Drag to reorder"
        >
          <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 24 24">
            <path d="M8 6h2v2H8V6zm6 0h2v2h-2V6zM8 11h2v2H8v-2zm6 0h2v2h-2v-2zm-6 5h2v2H8v-2zm6 0h2v2h-2v-2z" />
          </svg>
        </button>
        <span className="text-xs font-mono text-neutral-500 w-5">#{index + 1}</span>
        <ToolAutocomplete
          value={step.tool}
          tools={tools}
          onChange={handleToolChange}
        />
        <button
          onClick={onRemove}
          className="ml-auto text-neutral-600 hover:text-rose-400 transition-colors"
          title="Remove step"
        >
          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2">
            <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
          </svg>
        </button>
      </div>

      <input
        type="text"
        value={step.description ?? ""}
        onChange={(e) => onChange({ description: e.target.value || undefined })}
        placeholder="Step description (optional)"
        className="w-full px-3 py-1.5 rounded-md bg-neutral-950/60 border border-neutral-800 text-neutral-400 text-xs placeholder-neutral-600 focus:outline-none focus:border-emerald-500/50 transition-colors mb-3"
      />

      {schemaProps && Object.keys(schemaProps).length > 0 && (
        <DynamicForm
          properties={schemaProps as Record<string, unknown>}
          required={schemaRequired}
          values={step.params as Record<string, unknown>}
          onChange={(values) => onChange({ params: values })}
          variableNames={varNames}
          substitute={substitute}
        />
      )}
      {(!schemaProps || Object.keys(schemaProps).length === 0) && (
        <p className="text-[11px] text-neutral-500 italic">No parameters required</p>
      )}
    </div>
  );
}

function ToolAutocomplete({
  value,
  tools,
  onChange,
}: {
  value: string;
  tools: ToolOption[];
  onChange: (v: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState(value);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setSearch(value);
  }, [value]);

  useEffect(() => {
    const handleClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, []);

  const filtered = tools.filter(
    (t) =>
      t.name.toLowerCase().includes(search.toLowerCase()) ||
      (t.description ?? "").toLowerCase().includes(search.toLowerCase()),
  );
  const selectedTool = tools.find((t) => t.name === value);

  return (
    <div className="relative flex-1" ref={ref}>
      <input
        type="text"
        value={search}
        onChange={(e) => {
          setSearch(e.target.value);
          onChange(e.target.value);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        placeholder={tools.length > 0 ? "search tool..." : "loading..."}
        className="w-full px-3 py-1.5 rounded-md bg-neutral-950/60 border border-neutral-800 text-neutral-200 text-sm font-mono placeholder-neutral-600 focus:outline-none focus:border-emerald-500/50 transition-colors"
      />
      {selectedTool && value === search && !open && (
        <div className="absolute right-2 top-1/2 -translate-y-1/2">
          <span className="text-[10px] text-neutral-500 font-sans truncate max-w-[120px] block text-right">
            {selectedTool.description ?? ""}
          </span>
        </div>
      )}
      {open && filtered.length > 0 && (
        <div className="absolute z-10 top-full mt-1 left-0 right-0 bg-neutral-900 border border-neutral-700 rounded-md shadow-lg max-h-48 overflow-y-auto">
          {filtered.map((o) => (
            <button
              key={o.name}
              onMouseDown={() => {
                onChange(o.name);
                setSearch(o.name);
                setOpen(false);
              }}
              className={`w-full text-left px-3 py-2 text-sm font-mono transition-colors flex flex-col gap-0.5 ${
                o.name === value ? "text-emerald-300 bg-emerald-500/5" : "text-neutral-300 hover:bg-neutral-800"
              }`}
            >
              <span>{o.name}</span>
              {o.description && (
                <span className="text-[10px] text-neutral-500 font-sans line-clamp-1">{o.description}</span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
