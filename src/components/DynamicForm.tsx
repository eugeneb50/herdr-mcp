import { useCallback, useEffect, useRef, useState } from "react";

type Props = {
  properties: Record<string, unknown>;
  required: string[];
  values: Record<string, unknown>;
  onChange: (values: Record<string, unknown>) => void;
  variableNames?: string[];
  substitute?: (template: string) => string;
};

export function DynamicForm({ properties, required, values, onChange, variableNames, substitute }: Props) {
  const set = useCallback(
    (key: string, value: unknown) => {
      onChange({ ...values, [key]: value });
    },
    [values, onChange],
  );

  return (
    <div className="space-y-3">
      <label className="block text-xs font-mono uppercase tracking-wider text-neutral-500">Parameters</label>
      {Object.entries(properties).map(([key, val]) => {
        const prop = val as Record<string, unknown>;
        const isRequired = required.includes(key);
        const type = (prop.type as string) ?? "string";
        const desc = (prop.description as string) ?? "";

        return (
          <div key={key}>
            <label className="flex items-center gap-2 text-xs text-neutral-400 mb-1">
              <code className="font-mono text-emerald-300">{key}</code>
              <span className="text-neutral-600 font-mono">{type}</span>
              {isRequired && <span className="text-rose-400">required</span>}
            </label>
            {desc && <p className="text-[11px] text-neutral-500 mb-1">{desc}</p>}
            {renderInput(key, prop, values[key], (v) => set(key, v), isRequired, variableNames, substitute)}
          </div>
        );
      })}
    </div>
  );
}

function renderInput(
  key: string,
  prop: Record<string, unknown>,
  value: unknown,
  onChange: (v: unknown) => void,
  required: boolean,
  variableNames?: string[],
  substituteFn?: (template: string) => string,
) {
  const type = (prop.type as string) ?? "string";

  if (type === "boolean") {
    return (
      <label className="flex items-center gap-2 cursor-pointer">
        <input
          type="checkbox"
          checked={!!value}
          onChange={(e) => onChange(e.target.checked)}
          className="rounded bg-neutral-800 border-neutral-600 text-emerald-500 focus:ring-emerald-500/30"
        />
        <span className="text-xs text-neutral-400">enabled</span>
      </label>
    );
  }

  if (type === "integer" || type === "number") {
    return (
      <input
        type="number"
        value={(value as string) ?? ""}
        onChange={(e) => onChange(e.target.value ? Number(e.target.value) : undefined)}
        placeholder={required ? "required" : "optional"}
        className="w-full px-3 py-2 rounded-lg bg-neutral-900 border border-neutral-700 text-neutral-200 text-sm placeholder-neutral-600 focus:outline-none focus:border-emerald-500/50 transition-colors"
      />
    );
  }

  if (type === "array") {
    const items = (prop.items as Record<string, unknown>) ?? {};
    if (items.type === "string") {
      return (
        <input
          type="text"
          value={(value as string) ?? ""}
          onChange={(e) =>
            onChange(
              e.target.value
                ? e.target.value.split(",").map((s: string) => s.trim())
                : [],
            )
          }
          placeholder="comma-separated values"
          className="w-full px-3 py-2 rounded-lg bg-neutral-900 border border-neutral-700 text-neutral-200 text-sm placeholder-neutral-600 focus:outline-none focus:border-emerald-500/50 transition-colors"
        />
      );
    }
    return (
      <input
        type="text"
        value={JSON.stringify(value ?? "")}
        disabled
        className="w-full px-3 py-2 rounded-lg bg-neutral-900 border border-neutral-700 text-neutral-500 text-sm cursor-not-allowed"
      />
    );
  }

  const isLong = (prop.description as string ?? "").length > 60;

  const strValue = value != null ? String(value) : "";

  if (isLong) {
    return (
      <VariableInput
        value={strValue}
        onChange={(v) => onChange(v || undefined)}
        placeholder={required ? "required" : "optional"}
        multiline
        variableNames={variableNames}
        substituteFn={substituteFn}
      />
    );
  }

  return (
    <VariableInput
      value={strValue}
      onChange={(v) => onChange(v || undefined)}
      placeholder={required ? "required" : "optional"}
      variableNames={variableNames}
      substituteFn={substituteFn}
    />
  );
}

function VariableInput({
  value,
  onChange,
  placeholder,
  multiline,
  variableNames = [],
  substituteFn,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  multiline?: boolean;
  variableNames?: string[];
  substituteFn?: (template: string) => string;
}) {
  const [showDropdown, setShowDropdown] = useState(false);
  const [cursorPos, setCursorPos] = useState(0);
  const [filter, setFilter] = useState("");
  const [showPreview, setShowPreview] = useState(false);
  const inputRef = useRef<HTMLInputElement | HTMLTextAreaElement>(null);
  const dropdownRef = useRef<HTMLDivElement>(null);

  const varCount = (typeof value === "string" ? value.match(/\{\{/g) : [])?.length ?? 0;
  const resolved = substituteFn ? substituteFn(value) : value;
  const hasVars = varCount > 0;

  const filtered = variableNames.filter((n) => n.includes(filter));

  useEffect(() => {
    const handleClick = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setShowDropdown(false);
      }
    };
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, []);

  const handleChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => {
      const newVal = e.target.value;
      onChange(newVal);

      // Check if we just typed {{
      const pos = e.target.selectionStart ?? newVal.length;
      const before = newVal.slice(0, pos);
      const openIdx = before.lastIndexOf("{{");
      if (openIdx !== -1 && before.indexOf("}}", openIdx) === -1) {
        const afterOpen = before.slice(openIdx + 2);
        if (!afterOpen.includes(" ")) {
          setFilter(afterOpen);
          setShowDropdown(true);
          setCursorPos(pos);
          return;
        }
      }
      setShowDropdown(false);
    },
    [onChange],
  );

  const insertVar = useCallback(
    (name: string) => {
      const before = value.slice(0, cursorPos);
      const openIdx = before.lastIndexOf("{{");
      if (openIdx === -1) return;
      const after = value.slice(cursorPos);
      const newVal = before.slice(0, openIdx) + `{{${name}}}` + after;
      onChange(newVal);
      setShowDropdown(false);
      // Restore focus
      setTimeout(() => {
        const el = inputRef.current;
        if (el) {
          const newPos = openIdx + name.length + 4;
          el.focus();
          el.setSelectionRange(newPos, newPos);
        }
      }, 0);
    },
    [value, cursorPos, onChange],
  );

  const commonProps = {
    ref: inputRef as React.Ref<any>,
    value,
    onChange: handleChange,
    placeholder,
    onFocus: () => {
      if (value.includes("{{")) setShowPreview(true);
    },
    onBlur: () => setTimeout(() => setShowPreview(false), 200),
    className:
      "w-full px-3 py-2 rounded-lg bg-neutral-900 border text-sm placeholder-neutral-600 focus:outline-none transition-colors " +
      (hasVars
        ? "border-emerald-500/40 text-emerald-100"
        : "border-neutral-700 text-neutral-200 focus:border-emerald-500/50"),
  };

  return (
    <div className="relative" onMouseEnter={() => hasVars && setShowPreview(true)} onMouseLeave={() => setShowPreview(false)}>
      <div className="relative">
        {multiline ? (
          <textarea {...commonProps} rows={3} className={commonProps.className + " resize-none"} />
        ) : (
          <input {...commonProps} />
        )}
        {hasVars && (
          <div className="absolute right-2 top-1/2 -translate-y-1/2 flex items-center gap-1">
            <span className="px-1.5 py-0.5 text-[10px] font-mono bg-emerald-500/20 text-emerald-300 rounded">
              {varCount} var{varCount > 1 ? "s" : ""}
            </span>
          </div>
        )}
      </div>

      {hasVars && showPreview && resolved !== value && (
        <div className="absolute z-20 left-0 right-0 top-full mt-1 p-2 rounded-md bg-neutral-800 border border-neutral-700 shadow-lg">
          <div className="text-[10px] font-mono uppercase tracking-wider text-neutral-500 mb-1">Resolved</div>
          <div className="text-xs font-mono text-neutral-200 whitespace-pre-wrap break-all max-h-24 overflow-y-auto">
            {resolved}
          </div>
        </div>
      )}

      {showDropdown && filtered.length > 0 && (
        <div
          ref={dropdownRef}
          className="absolute z-30 left-0 right-0 top-full mt-1 bg-neutral-900 border border-emerald-500/30 rounded-md shadow-lg max-h-40 overflow-y-auto"
        >
          {filtered.map((name) => (
            <button
              key={name}
              onMouseDown={(e) => {
                e.preventDefault();
                insertVar(name);
              }}
              className="w-full text-left px-3 py-1.5 text-xs font-mono text-neutral-200 hover:bg-emerald-500/10 transition-colors flex items-center gap-2"
            >
              <span className="text-emerald-400">{name}</span>
              <span className="text-neutral-500 truncate">= {substituteFn ? substituteFn(`{{${name}}}`) : ""}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
