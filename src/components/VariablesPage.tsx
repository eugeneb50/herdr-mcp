import { useCallback, useRef, useState } from "react";
import { useVariables } from "./VariableStore";

export function VariablesPage() {
  const { userVars, autoVars, setUserVar, removeUserVar, clearUserVars, clearAutoVars, userVarNames, groupedAutoVars, substitute } = useVariables();
  const [tab, setTab] = useState<"user" | "auto">("user");
  const [search, setSearch] = useState("");

  return (
    <div className="max-w-4xl mx-auto px-6 py-12">
      <div className="mb-8">
        <div className="text-xs font-mono text-emerald-400 uppercase tracking-wider mb-3">Variables</div>
        <h1 className="text-3xl md:text-4xl font-bold text-neutral-50 tracking-tight mb-4">
          Variable store
        </h1>
        <p className="text-neutral-400 leading-relaxed max-w-2xl">
          <strong className="text-emerald-300">User</strong> variables persist until you edit or delete them.
          <strong className="text-emerald-300 ml-3">Auto</strong> variables are overwritten each time a tool runs.
          User variables take precedence over auto variables with the same name.
        </p>
      </div>

      <div className="flex gap-1 mb-6 border-b border-neutral-800/60">
        <button
          onClick={() => setTab("user")}
          className={`px-4 py-2.5 text-sm font-mono transition-colors border-b-2 -mb-[1px] ${
            tab === "user" ? "border-emerald-400 text-neutral-100" : "border-transparent text-neutral-500 hover:text-neutral-300"
          }`}
        >
          User Variables ({userVarNames.length})
        </button>
        <button
          onClick={() => setTab("auto")}
          className={`px-4 py-2.5 text-sm font-mono transition-colors border-b-2 -mb-[1px] ${
            tab === "auto" ? "border-emerald-400 text-neutral-100" : "border-transparent text-neutral-500 hover:text-neutral-300"
          }`}
        >
          Auto Variables ({Object.keys(autoVars).length})
        </button>
      </div>

      <div className="relative mb-6 max-w-md">
        <svg className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-neutral-500" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2">
          <path strokeLinecap="round" strokeLinejoin="round" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
        </svg>
        <input
          type="text"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Filter variables..."
          className="w-full pl-10 pr-4 py-2.5 rounded-lg bg-neutral-900 border border-neutral-700 text-neutral-200 text-sm placeholder-neutral-600 focus:outline-none focus:border-emerald-500/50 transition-colors"
        />
      </div>

      {tab === "user" && <UserVarsTable search={search} />}
      {tab === "auto" && <AutoVarsTable search={search} />}
    </div>
  );
}

function UserVarsTable({ search }: { search: string }) {
  const { userVars, setUserVar, removeUserVar, clearUserVars, userVarNames, substitute } = useVariables();
  const [editing, setEditing] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [editValue, setEditValue] = useState("");
  const [adding, setAdding] = useState(false);
  const [newName, setNewName] = useState("");
  const [newValue, setNewValue] = useState("");
  const [copied, setCopied] = useState<string | null>(null);
  const [importText, setImportText] = useState("");
  const [showImport, setShowImport] = useState(false);

  const copyName = useCallback((name: string) => {
    navigator.clipboard.writeText(`{{${name}}}`).then(() => {
      setCopied(name);
      setTimeout(() => setCopied(null), 1200);
    });
  }, []);

  const handleEdit = useCallback((oldName: string) => {
    if (editName !== oldName) {
      removeUserVar(oldName);
    }
    setUserVar(editName, editValue);
    setEditing(null);
  }, [editName, editValue, setUserVar, removeUserVar]);

  const handleAdd = useCallback(() => {
    const n = newName.trim();
    if (!n) return;
    setUserVar(n, newValue);
    setNewName("");
    setNewValue("");
    setAdding(false);
  }, [newName, newValue, setUserVar]);

  const handleImport = useCallback(() => {
    try {
      const parsed = JSON.parse(importText) as Record<string, string>;
      for (const [k, v] of Object.entries(parsed)) {
        setUserVar(k, String(v));
      }
      setShowImport(false);
      setImportText("");
    } catch {
      alert("Invalid JSON. Expected { \"name\": \"value\", ... }");
    }
  }, [importText, setUserVar]);

  const handleExport = useCallback(() => {
    const blob = new Blob([JSON.stringify(userVars, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "herdr-mcp-user-vars.json";
    a.click();
    URL.revokeObjectURL(url);
  }, [userVars]);

  const filtered = userVarNames.filter((n) => n.includes(search) || userVars[n].includes(search));

  return (
    <div>
      <div className="mb-4 flex gap-2">
        <button onClick={() => setAdding(true)} className="px-3 py-1.5 rounded-lg border border-emerald-500/30 bg-emerald-500/5 text-emerald-300 text-xs font-mono hover:bg-emerald-500/10 transition-colors">+ Add</button>
        <button onClick={() => setShowImport(!showImport)} className="px-3 py-1.5 rounded-lg border border-neutral-700 text-neutral-400 text-xs font-mono hover:text-neutral-200 transition-colors">Import</button>
        <button onClick={handleExport} disabled={userVarNames.length === 0} className="px-3 py-1.5 rounded-lg border border-neutral-700 text-neutral-400 text-xs font-mono hover:text-neutral-200 disabled:opacity-40 disabled:cursor-not-allowed transition-colors">Export</button>
        <button onClick={clearUserVars} disabled={userVarNames.length === 0} className="px-3 py-1.5 rounded-lg border border-rose-500/20 text-rose-400 text-xs font-mono hover:bg-rose-500/10 disabled:opacity-40 disabled:cursor-not-allowed transition-colors ml-auto">Clear all</button>
      </div>

      {showImport && (
        <div className="mb-4 p-4 rounded-lg border border-neutral-800 bg-neutral-900/40">
          <textarea
            value={importText}
            onChange={(e) => setImportText(e.target.value)}
            placeholder='{"var_name": "value", ...}'
            rows={3}
            className="w-full px-3 py-2 rounded-lg bg-neutral-950 border border-neutral-700 text-neutral-200 text-xs font-mono placeholder-neutral-600 focus:outline-none focus:border-emerald-500/50 transition-colors resize-none"
          />
          <div className="flex gap-2 mt-2">
            <button onClick={handleImport} className="px-3 py-1 rounded bg-emerald-500 text-neutral-950 text-xs font-semibold hover:bg-emerald-400 transition-colors">Import</button>
            <button onClick={() => setShowImport(false)} className="px-3 py-1 rounded border border-neutral-700 text-neutral-400 text-xs hover:text-neutral-200 transition-colors">Cancel</button>
          </div>
        </div>
      )}

      {filtered.length === 0 && (
        <p className="text-neutral-500 text-sm py-8 text-center italic">
          {search ? `No variables matching "${search}"` : "No user variables yet. Add one or run a tool to auto-populate."}
        </p>
      )}

      <div className="space-y-1">
        {filtered.map((name) => {
          const resolved = substitute(`{{${name}}}`);
          const isActive = editing === name;
          return (
            <div
              key={name}
              className="group flex items-center gap-3 px-3 py-2 rounded-lg hover:bg-neutral-900/60 transition-colors"
            >
              {isActive ? (
                <>
                  <input
                    type="text"
                    value={editName}
                    onChange={(e) => setEditName(e.target.value)}
                    className="w-40 px-2 py-1 rounded bg-neutral-950 border border-neutral-700 text-emerald-300 text-xs font-mono focus:outline-none focus:border-emerald-500/50"
                    autoFocus
                  />
                  <input
                    type="text"
                    value={editValue}
                    onChange={(e) => setEditValue(e.target.value)}
                    className="flex-1 px-2 py-1 rounded bg-neutral-950 border border-neutral-700 text-neutral-200 text-xs font-mono focus:outline-none focus:border-emerald-500/50"
                  />
                  <button onClick={() => handleEdit(name)} className="text-emerald-400 hover:text-emerald-300 text-xs">Save</button>
                  <button onClick={() => setEditing(null)} className="text-neutral-500 hover:text-neutral-300 text-xs">Cancel</button>
                </>
              ) : (
                <>
                  <button
                    onClick={() => copyName(name)}
                    className="flex-1 flex items-center gap-3 min-w-0 text-left"
                    title={`Click to copy {{${name}}}`}
                  >
                    <code className="font-mono text-xs text-emerald-300 w-36 truncate flex-shrink-0">{name}</code>
                    <span className="text-neutral-400 font-mono text-xs truncate">{resolved}</span>
                    {copied === name && <span className="text-emerald-400 text-[10px] flex-shrink-0">copied!</span>}
                  </button>
                  <button
                    onClick={() => { setEditing(name); setEditName(name); setEditValue(userVars[name]); }}
                    className="text-neutral-600 hover:text-neutral-300 opacity-0 group-hover:opacity-100 transition-opacity text-xs"
                  >
                    Edit
                  </button>
                  <button
                    onClick={() => removeUserVar(name)}
                    className="text-neutral-600 hover:text-rose-400 opacity-0 group-hover:opacity-100 transition-opacity text-xs"
                  >
                    Delete
                  </button>
                </>
              )}
            </div>
          );
        })}
      </div>

      {adding && (
        <div className="mt-4 p-4 rounded-lg border border-neutral-800 bg-neutral-900/40 space-y-2">
          <input
            type="text"
            value={newName}
            onChange={(e) => setNewName(e.target.value.replace(/[^a-zA-Z0-9_.]/g, "_"))}
            placeholder="variable_name"
            className="w-full px-3 py-2 rounded-lg bg-neutral-950 border border-neutral-700 text-neutral-200 text-sm font-mono placeholder-neutral-600 focus:outline-none focus:border-emerald-500/50 transition-colors"
          />
          <input
            type="text"
            value={newValue}
            onChange={(e) => setNewValue(e.target.value)}
            placeholder="value"
            className="w-full px-3 py-2 rounded-lg bg-neutral-950 border border-neutral-700 text-neutral-200 text-sm font-mono placeholder-neutral-600 focus:outline-none focus:border-emerald-500/50 transition-colors"
            onKeyDown={(e) => { if (e.key === "Enter") handleAdd(); if (e.key === "Escape") setAdding(false); }}
            autoFocus
          />
          <div className="flex gap-2">
            <button onClick={handleAdd} className="px-3 py-1.5 rounded bg-emerald-500 text-neutral-950 text-xs font-semibold hover:bg-emerald-400 transition-colors">Add</button>
            <button onClick={() => setAdding(false)} className="px-3 py-1.5 rounded border border-neutral-700 text-neutral-400 text-xs hover:text-neutral-200 transition-colors">Cancel</button>
          </div>
        </div>
      )}
    </div>
  );
}

function AutoVarsTable({ search }: { search: string }) {
  const { autoVars, clearAutoVars, groupedAutoVars } = useVariables();
  const [copied, setCopied] = useState<string | null>(null);

  const copyName = useCallback((name: string) => {
    navigator.clipboard.writeText(`{{${name}}}`).then(() => {
      setCopied(name);
      setTimeout(() => setCopied(null), 1200);
    });
  }, []);

  const filtered = groupedAutoVars.filter(([k, v]) => k.includes(search) || v.includes(search));

  const groups: Record<string, [string, string][]> = {};
  const ungrouped: [string, string][] = [];
  for (const [k, v] of filtered) {
    const dot = k.indexOf(".");
    if (dot > 0) {
      const prefix = k.slice(0, dot);
      (groups[prefix] ??= []).push([k.slice(dot + 1), v]);
    } else {
      ungrouped.push([k, v]);
    }
  }

  return (
    <div>
      <div className="mb-4 flex gap-2">
        <button onClick={clearAutoVars} disabled={Object.keys(autoVars).length === 0} className="px-3 py-1.5 rounded-lg border border-rose-500/20 text-rose-400 text-xs font-mono hover:bg-rose-500/10 disabled:opacity-40 disabled:cursor-not-allowed transition-colors ml-auto">Clear all</button>
      </div>

      {filtered.length === 0 && (
        <p className="text-neutral-500 text-sm py-8 text-center italic">
          {search ? `No variables matching "${search}"` : "No auto variables yet. Run a tool to populate them."}
        </p>
      )}

      <div className="space-y-6">
        {Object.entries(groups).sort((a, b) => a[0].localeCompare(b[0])).map(([prefix, children]) => (
          <div key={prefix}>
            <div className="text-xs font-mono uppercase tracking-wider text-teal-500/80 mb-2 px-3">{prefix}</div>
            <div className="space-y-0.5">
              {children.map(([key, val]) => (
                <div
                  key={`${prefix}.${key}`}
                  className="group flex items-center gap-3 px-3 py-1.5 rounded-lg hover:bg-neutral-900/60 transition-colors"
                >
                  <button
                    onClick={() => copyName(`${prefix}.${key}`)}
                    className="flex-1 flex items-center gap-3 min-w-0 text-left"
                  >
                    <code className="font-mono text-xs text-teal-300 w-28 truncate flex-shrink-0">{key}</code>
                    <span className="text-neutral-400 font-mono text-xs truncate">{val}</span>
                    {copied === `${prefix}.${key}` && <span className="text-emerald-400 text-[10px] flex-shrink-0">copied!</span>}
                  </button>
                </div>
              ))}
            </div>
          </div>
        ))}

        {ungrouped.length > 0 && (
          <div>
            <div className="text-xs font-mono uppercase tracking-wider text-neutral-600 mb-2 px-3">Aliases</div>
            <div className="space-y-0.5">
              {ungrouped.map(([key, val]) => (
                <div
                  key={key}
                  className="group flex items-center gap-3 px-3 py-1.5 rounded-lg hover:bg-neutral-900/60 transition-colors"
                >
                  <button
                    onClick={() => copyName(key)}
                    className="flex-1 flex items-center gap-3 min-w-0 text-left"
                  >
                    <code className="font-mono text-xs text-emerald-300 w-28 truncate flex-shrink-0">{key}</code>
                    <span className="text-neutral-400 font-mono text-xs truncate">{val}</span>
                    {copied === key && <span className="text-emerald-400 text-[10px] flex-shrink-0">copied!</span>}
                  </button>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
