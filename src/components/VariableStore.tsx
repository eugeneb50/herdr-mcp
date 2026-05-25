import { createContext, useCallback, useContext, useMemo, useState } from "react";
import type { ReactNode } from "react";

type VarNamespace = "user" | "auto";
type Vars = Record<string, string>;
type AllVars = Record<VarNamespace, Vars>;

type ExtractFn = (toolName: string, response: unknown) => void;

type Ctx = {
  variables: Vars;           // merged: user first, auto fallback
  userVars: Vars;
  autoVars: Vars;
  setUserVar: (name: string, value: string) => void;
  setUserVars: (vars: Vars) => void;
  removeUserVar: (name: string) => void;
  clearUserVars: () => void;
  clearAutoVars: () => void;
  substitute: (template: string) => string;
  extractFromResponse: ExtractFn;
  groupedAutoVars: [string, string][]; // sorted for display
  userVarNames: string[];
  allVarNames: string[];
  varNames: string[];  // alias for allVarNames
};

const STORAGE_KEY = "herdr_mcp_vars_v2";

function load(): AllVars {
  try {
    return JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{"user":{},"auto":{}}');
  } catch {
    return { user: {}, auto: {} };
  }
}

function save(v: AllVars) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(v));
  } catch { /* quota */ }
}

function mergeVars(all: AllVars): Vars {
  return { ...all.auto, ...all.user };
}

const Ctx = createContext<Ctx | null>(null);

export function VariableProvider({ children }: { children: ReactNode }) {
  const [allVars, setAllVars] = useState<AllVars>(load);

  const persist = useCallback((v: AllVars) => {
    setAllVars(v);
    save(v);
  }, []);

  const setUserVar = useCallback((name: string, value: string) => {
    setAllVars((prev) => {
      const next = { user: { ...prev.user, [name]: value }, auto: prev.auto };
      save(next);
      return next;
    });
  }, []);

  const setUserVars = useCallback((vars: Vars) => {
    setAllVars((prev) => {
      const next = { user: { ...prev.user, ...vars }, auto: prev.auto };
      save(next);
      return next;
    });
  }, []);

  const removeUserVar = useCallback((name: string) => {
    setAllVars((prev) => {
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { [name]: _, ...rest } = prev.user;
      const next = { user: rest, auto: prev.auto };
      save(next);
      return next;
    });
  }, []);

  const clearUserVars = useCallback(() => {
    setAllVars((prev) => {
      const next = { user: {}, auto: prev.auto };
      save(next);
      return next;
    });
  }, []);

  const clearAutoVars = useCallback(() => {
    setAllVars((prev) => {
      const next = { user: prev.user, auto: {} };
      save(next);
      return next;
    });
  }, []);

  const substitute = useCallback(
    (template: string): string => {
      return template.replace(/\{\{(\w[\w.]*\w|\w)\}\}/g, (_match, name: string) => {
        return allVars.user[name] ?? allVars.auto[name] ?? `{{${name}}}`;
      });
    },
    [allVars],
  );

  // ── Grouped Auto-Extraction ────────────────────────────────────────────

  const extractFromResponse = useCallback(
    (toolName: string, response: unknown) => {
      const updates: Vars = {};

      const d = response as Record<string, unknown>;
      const result = (d.result ?? d) as Record<string, unknown>;
      const content = (result.content ?? []) as Array<Record<string, unknown>>;

      // Parse first text/JSON content
      let parsed: Record<string, unknown> | null = null;
      let rawText = "";
      for (const c of content) {
        const text = c.text as string;
        if (c.type === "text" && text) {
          rawText = text;
          try {
            const j = JSON.parse(text) as Record<string, unknown>;
            if (j && typeof j === "object") { parsed = j; break; }
          } catch { /* not JSON */ }
        }
      }

      updates.last_result = JSON.stringify(response ?? "", null, 2).slice(0, 2000);

      // We only auto-extract from parsable JSON
      if (!parsed) {
        // Non-JSON tools: read_pane, read_agent, split_pane, create_*, etc.
        if (rawText) {
          if (toolName === "read_pane" || toolName === "read_agent") {
            updates.output_text = rawText;
            updates.pane_output = rawText;
            // Extract file paths
            const paths: string[] = [];
            const re = /\/[\w/.-]+/g;
            let m: RegExpExecArray | null;
            while ((m = re.exec(rawText)) !== null && paths.length < 10) {
              paths.push(m[0].trim());
            }
            paths.forEach((p, i) => { updates[`file_path_${i + 1}`] = p; });
          }
          if (toolName === "split_pane") { updates.pane_id = rawText.trim(); updates["pane_1.pane_id"] = rawText.trim(); }
          if (toolName === "create_workspace") { updates.workspace_id = rawText.trim(); updates["workspace_1.workspace_id"] = rawText.trim(); }
          if (toolName === "create_tab") { updates.tab_id = rawText.trim(); updates["tab_1.tab_id"] = rawText.trim(); }
        }
        return flushUpdates(updates);
      }

      const inner = (parsed.result ?? parsed) as Record<string, unknown>;

      // ── list_panes ──────────────────────────────────────────────────
      if (toolName === "list_panes") {
        const arr = (inner.panes as Array<Record<string, unknown>>) ?? [];
        for (let i = 0; i < Math.min(arr.length, 10); i++) {
          const p = arr[i];
          const prefix = `pane_${i + 1}`;
          if (p.pane_id) { updates[`${prefix}.pane_id`] = String(p.pane_id); }
          if (p.label) { updates[`${prefix}.label`] = String(p.label); }
          if (p.cwd) { updates[`${prefix}.cwd`] = String(p.cwd); }
          if (p.agent_status) { updates[`${prefix}.agent_status`] = String(p.agent_status); }
        }
        if (arr[0]?.pane_id) updates.pane_id = String(arr[0].pane_id);
      }

      // ── list_workspaces ─────────────────────────────────────────────
      if (toolName === "list_workspaces") {
        const arr = (inner.workspaces as Array<Record<string, unknown>>) ?? [];
        for (let i = 0; i < Math.min(arr.length, 10); i++) {
          const w = arr[i];
          const prefix = `workspace_${i + 1}`;
          if (w.workspace_id) { updates[`${prefix}.workspace_id`] = String(w.workspace_id); }
          if (w.label) { updates[`${prefix}.label`] = String(w.label); }
          if (w.agent_status) { updates[`${prefix}.agent_status`] = String(w.agent_status); }
        }
        if (arr[0]?.workspace_id) updates.workspace_id = String(arr[0].workspace_id);
      }

      // ── list_tabs ───────────────────────────────────────────────────
      if (toolName === "list_tabs") {
        const arr = (inner.tabs as Array<Record<string, unknown>>) ?? [];
        for (let i = 0; i < Math.min(arr.length, 10); i++) {
          const t = arr[i];
          const prefix = `tab_${i + 1}`;
          if (t.tab_id) { updates[`${prefix}.tab_id`] = String(t.tab_id); }
          if (t.label) { updates[`${prefix}.label`] = String(t.label); }
        }
        if (arr[0]?.tab_id) updates.tab_id = String(arr[0].tab_id);
      }

      // ── list_agents ─────────────────────────────────────────────────
      if (toolName === "list_agents") {
        const arr = (inner.agents as Array<Record<string, unknown>>) ?? [];
        for (let i = 0; i < Math.min(arr.length, 10); i++) {
          const a = arr[i];
          const prefix = `agent_${i + 1}`;
          if (a.target) { updates[`${prefix}.target`] = String(a.target); }
          if (a.name) { updates[`${prefix}.name`] = String(a.name); }
          if (a.status) { updates[`${prefix}.status`] = String(a.status); }
          if (a.cwd) { updates[`${prefix}.cwd`] = String(a.cwd); }
        }
        if (arr[0]?.target) updates.target = String(arr[0].target);
      }

      // ── get_pane ────────────────────────────────────────────────────
      if (toolName === "get_pane") {
        const prefix = "pane";
        if (inner.pane_id) { updates[`${prefix}.pane_id`] = String(inner.pane_id); updates.pane_id = String(inner.pane_id); }
        if (inner.cwd) { updates[`${prefix}.cwd`] = String(inner.cwd); updates.pane_cwd = String(inner.cwd); }
        if (inner.agent_status) { updates[`${prefix}.agent_status`] = String(inner.agent_status); updates.pane_agent_status = String(inner.agent_status); }
        if (inner.label) { updates[`${prefix}.label`] = String(inner.label); }
      }

      // ── get_agent ───────────────────────────────────────────────────
      if (toolName === "get_agent") {
        const prefix = "agent";
        if (inner.target) { updates[`${prefix}.target`] = String(inner.target); updates.agent_target = String(inner.target); }
        if (inner.status) { updates[`${prefix}.status`] = String(inner.status); updates.agent_status = String(inner.status); }
        if (inner.name) { updates[`${prefix}.name`] = String(inner.name); }
        if (inner.cwd) { updates[`${prefix}.cwd`] = String(inner.cwd); }
      }

      // ── status ──────────────────────────────────────────────────────
      if (toolName === "status") {
        if (inner.server_status) { updates.server_status = String(inner.server_status); }
        if (inner.client_status) { updates.client_status = String(inner.client_status); }
      }

      flushUpdates(updates);
    },
    [],
  );

  function flushUpdates(updates: Vars) {
    if (Object.keys(updates).length === 0) return;
    setAllVars((prev) => {
      const next = { user: prev.user, auto: { ...prev.auto, ...updates } };
      save(next);
      return next;
    });
  }

  // ── Derived state ──────────────────────────────────────────────────────

  const variables = useMemo(() => mergeVars(allVars), [allVars]);
  const { user: userVars, auto: autoVars } = allVars;

  const groupedAutoVars = useMemo(() => {
    const entries = Object.entries(autoVars).filter(([k]) => k !== "last_result");
    entries.sort((a, b) => a[0].localeCompare(b[0]));
    return entries;
  }, [autoVars]);

  const userVarNames = useMemo(() => Object.keys(userVars), [userVars]);
  const allVarNames = useMemo(() => [...new Set([...Object.keys(userVars), ...Object.keys(autoVars)])], [userVars, autoVars]);

  const value = useMemo<Ctx>(
    () => ({
      variables,
      userVars,
      autoVars,
      setUserVar,
      setUserVars,
      removeUserVar,
      clearUserVars,
      clearAutoVars,
      substitute,
      extractFromResponse,
      groupedAutoVars,
      userVarNames,
      allVarNames,
      varNames: allVarNames,
    }),
    [variables, userVars, autoVars, setUserVar, setUserVars, removeUserVar, clearUserVars, clearAutoVars, substitute, extractFromResponse, groupedAutoVars, userVarNames, allVarNames],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useVariables(): Ctx {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useVariables must be used within VariableProvider");
  return ctx;
}
