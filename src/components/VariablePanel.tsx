import { useCallback, useRef, useState } from "react";
import { useVariables } from "./VariableStore";

export function VariablePanel() {
  const { userVars, autoVars, setUserVar, removeUserVar, clearUserVars, clearAutoVars, userVarNames } = useVariables();
  const [adding, setAdding] = useState(false);
  const [newName, setNewName] = useState("");
  const [newValue, setNewValue] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [editValue, setEditValue] = useState("");
  const [copied, setCopied] = useState<string | null>(null);
  const [tab, setTab] = useState<"user" | "auto">("user");
  const nameRef = useRef<HTMLInputElement>(null);

  const handleAdd = useCallback(() => {
    const name = newName.trim();
    if (!name) return;
    setUserVar(name, newValue);
    setNewName("");
    setNewValue("");
    setAdding(false);
  }, [newName, newValue, setUserVar]);

  const handleEdit = useCallback(
    (name: string) => {
      setUserVar(name, editValue);
      setEditing(null);
    },
    [editValue, setUserVar],
  );

  const copyName = useCallback((name: string) => {
    navigator.clipboard.writeText(`{{${name}}}`).then(() => {
      setCopied(name);
      setTimeout(() => setCopied(null), 1200);
    });
  }, []);

  const totalUser = userVarNames.length;
  const totalAuto = Object.keys(autoVars).length;

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <h3 className="text-xs font-mono uppercase tracking-wider text-neutral-500">
          Variables
        </h3>
        <div className="flex gap-1">
          <button
            onClick={() => setTab("user")}
            className={`px-2 py-0.5 text-[10px] font-mono rounded ${
              tab === "user" ? "bg-emerald-500/20 text-emerald-300" : "text-neutral-500 hover:text-neutral-300"
            }`}
          >
            User ({totalUser})
          </button>
          <button
            onClick={() => setTab("auto")}
            className={`px-2 py-0.5 text-[10px] font-mono rounded ${
              tab === "auto" ? "bg-emerald-500/20 text-emerald-300" : "text-neutral-500 hover:text-neutral-300"
            }`}
          >
            Auto ({totalAuto})
          </button>
        </div>
      </div>

      {tab === "user" && <UserVarSection />}
      {tab === "auto" && <AutoVarSection />}
    </div>
  );
}

function VarRow({
  name,
  value,
  onCopy,
  copied,
  onEdit,
  onDelete,
  editing,
  editValue,
  onEditChange,
  onEditSave,
  onEditCancel,
  preview,
}: {
  name: string;
  value: string;
  onCopy: (n: string) => void;
  copied: string | null;
  onEdit?: (n: string) => void;
  onDelete?: (n: string) => void;
  editing?: boolean;
  editValue?: string;
  onEditChange?: (v: string) => void;
  onEditSave?: () => void;
  onEditCancel?: () => void;
  preview?: string;
}) {
  const isDotPath = name.includes(".");
  return (
    <div className="group flex items-center gap-2 px-2 py-1.5 rounded-md hover:bg-neutral-800/60 transition-colors text-xs">
      <button
        onClick={() => onCopy(name)}
        className="flex-1 flex items-center gap-2 min-w-0 text-left"
        title={`Click to copy {{${name}}}`}
      >
        <code className={`font-mono w-28 truncate flex-shrink-0 ${isDotPath ? "text-teal-400" : "text-emerald-300"}`}>
          {name}
        </code>
        {editing ? (
          <input
            type="text"
            value={editValue}
            onChange={(e) => onEditChange?.(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") onEditSave?.();
              if (e.key === "Escape") onEditCancel?.();
            }}
            className="flex-1 px-2 py-0.5 rounded bg-neutral-950 border border-neutral-700 text-neutral-200 text-xs font-mono focus:outline-none focus:border-emerald-500/50"
            autoFocus
          />
        ) : (
          <span className="text-neutral-400 truncate font-mono text-[11px]">{preview ?? value}</span>
        )}
        {copied === name && <span className="text-emerald-400 text-[10px] flex-shrink-0">copied!</span>}
      </button>
      {onEdit && editing && (
        <>
          <button onClick={onEditSave} className="text-emerald-400 hover:text-emerald-300 flex-shrink-0">✓</button>
          <button onClick={onEditCancel} className="text-neutral-500 hover:text-neutral-300 flex-shrink-0">✕</button>
        </>
      )}
      {onEdit && !editing && (
        <button
          onClick={() => onEdit(name)}
          className="text-neutral-600 hover:text-neutral-300 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0"
          title="Edit"
        >
          ✏
        </button>
      )}
      {onDelete && (
        <button
          onClick={() => onDelete(name)}
          className="text-neutral-600 hover:text-rose-400 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0"
          title="Delete"
        >
          ✕
        </button>
      )}
    </div>
  );
}

function UserVarSection() {
  const { userVars, setUserVar, removeUserVar, clearUserVars, userVarNames } = useVariables();
  const [adding, setAdding] = useState(false);
  const [newName, setNewName] = useState("");
  const [newValue, setNewValue] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [editValue, setEditValue] = useState("");
  const [copied, setCopied] = useState<string | null>(null);
  const nameRef = useRef<HTMLInputElement>(null);

  const handleAdd = useCallback(() => {
    const n = newName.trim();
    if (!n) return;
    setUserVar(n, newValue);
    setNewName("");
    setNewValue("");
    setAdding(false);
  }, [newName, newValue, setUserVar]);

  const handleEdit = useCallback((name: string) => {
    setUserVar(name, editValue);
    setEditing(null);
  }, [editValue, setUserVar]);

  const copyName = useCallback((name: string) => {
    navigator.clipboard.writeText(`{{${name}}}`).then(() => {
      setCopied(name);
      setTimeout(() => setCopied(null), 1200);
    });
  }, []);

  return (
    <div className="space-y-1">
      {userVarNames.length === 0 && !adding && (
        <p className="text-[11px] text-neutral-500 italic">No user variables. Add one below or run a tool to auto-populate.</p>
      )}
      <div className="max-h-48 overflow-y-auto space-y-0.5">
        {userVarNames.map((name) => (
          <VarRow
            key={name}
            name={name}
            value={userVars[name]}
            onCopy={copyName}
            copied={copied}
            onEdit={(n) => { setEditing(n); setEditValue(userVars[n]); }}
            onDelete={removeUserVar}
            editing={editing === name}
            editValue={editValue}
            onEditChange={setEditValue}
            onEditSave={() => handleEdit(name)}
            onEditCancel={() => setEditing(null)}
          />
        ))}
      </div>

      {adding && (
        <div className="space-y-2 pt-2 border-t border-neutral-800/60">
          <input
            ref={nameRef}
            type="text"
            value={newName}
            onChange={(e) => setNewName(e.target.value.replace(/[^a-zA-Z0-9_.]/g, "_"))}
            placeholder="variable_name"
            className="w-full px-2 py-1.5 rounded-md bg-neutral-950 border border-neutral-700 text-neutral-200 text-xs font-mono placeholder-neutral-600 focus:outline-none focus:border-emerald-500/50"
          />
          <input
            type="text"
            value={newValue}
            onChange={(e) => setNewValue(e.target.value)}
            placeholder="value"
            className="w-full px-2 py-1.5 rounded-md bg-neutral-950 border border-neutral-700 text-neutral-200 text-xs font-mono placeholder-neutral-600 focus:outline-none focus:border-emerald-500/50"
            onKeyDown={(e) => {
              if (e.key === "Enter") handleAdd();
              if (e.key === "Escape") setAdding(false);
            }}
            autoFocus
          />
          <div className="flex gap-2">
            <button onClick={handleAdd} className="px-3 py-1 rounded bg-emerald-500 text-neutral-950 text-xs font-semibold hover:bg-emerald-400 transition-colors">Add</button>
            <button onClick={() => setAdding(false)} className="px-3 py-1 rounded border border-neutral-700 text-neutral-400 text-xs hover:text-neutral-200 transition-colors">Cancel</button>
          </div>
        </div>
      )}

      <div className="flex gap-2 pt-1">
        {!adding && (
          <button onClick={() => setAdding(true)} className="text-[11px] text-neutral-500 hover:text-neutral-300 transition-colors">+ Add variable</button>
        )}
        {userVarNames.length > 0 && (
          <button onClick={clearUserVars} className="text-[11px] text-neutral-600 hover:text-rose-400 transition-colors ml-auto">Clear user vars</button>
        )}
      </div>
    </div>
  );
}

function AutoVarSection() {
  const { autoVars, clearAutoVars, groupedAutoVars } = useVariables();
  const [copied, setCopied] = useState<string | null>(null);

  const copyName = useCallback((name: string) => {
    navigator.clipboard.writeText(`{{${name}}}`).then(() => {
      setCopied(name);
      setTimeout(() => setCopied(null), 1200);
    });
  }, []);

  // Group by prefix (e.g., pane_1, agent_2)
  const groups: Record<string, [string, string][]> = {};
  const ungrouped: [string, string][] = [];
  for (const [k, v] of groupedAutoVars) {
    const dot = k.indexOf(".");
    if (dot > 0) {
      const prefix = k.slice(0, dot);
      (groups[prefix] ??= []).push([k.slice(dot + 1), v]);
    } else {
      ungrouped.push([k, v]);
    }
  }

  return (
    <div className="space-y-2 max-h-64 overflow-y-auto">
      {Object.keys(groups).length === 0 && ungrouped.length === 0 && (
        <p className="text-[11px] text-neutral-500 italic">No auto variables yet. Run a tool to populate them.</p>
      )}

      {Object.entries(groups).sort((a, b) => a[0].localeCompare(b[0])).map(([prefix, children]) => (
        <div key={prefix}>
          <div className="text-[10px] font-mono uppercase tracking-wider text-teal-500/70 mb-0.5 px-1">{prefix}</div>
          {children.map(([key, val]) => (
            <VarRow
              key={`${prefix}.${key}`}
              name={`${prefix}.${key}`}
              value={val}
              onCopy={copyName}
              copied={copied}
            />
          ))}
        </div>
      ))}

      {ungrouped.length > 0 && (
        <div>
          <div className="text-[10px] font-mono uppercase tracking-wider text-neutral-600 mb-0.5 px-1">Aliases</div>
          {ungrouped.map(([key, val]) => (
            <VarRow
              key={key}
              name={key}
              value={val}
              onCopy={copyName}
              copied={copied}
            />
          ))}
        </div>
      )}

      <div className="flex gap-2 pt-1">
        {Object.keys(autoVars).length > 0 && (
          <button onClick={clearAutoVars} className="text-[11px] text-neutral-600 hover:text-rose-400 transition-colors">Clear auto vars</button>
        )}
      </div>
    </div>
  );
}
