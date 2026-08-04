import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { listVariables, saveVariable, deleteVariable, clearVariables } from "../api/variables";

type Vars = Record<string, string>;

type Ctx = {
  variables: Vars;           // merged: user vars (from server)
  userVars: Vars;
  autoVars: Vars;
  setUserVar: (name: string, value: string) => Promise<void>;
  setUserVars: (vars: Vars) => Promise<void>;
  removeUserVar: (name: string) => Promise<void>;
  clearUserVars: () => Promise<void>;
  clearAutoVars: () => void; // no-op, kept for compatibility
  substitute: (template: string) => string;
  groupedAutoVars: [string, string][]; // empty, kept for compatibility
  userVarNames: string[];
  allVarNames: string[];
  varNames: string[];  // alias for allVarNames
};

const Ctx = createContext<Ctx | null>(null);

export function VariableProvider({ children }: { children: ReactNode }) {
  const [userVars, setUserVarsState] = useState<Vars>({});
  const [autoVars, setAutoVarsState] = useState<Vars>({});

  useEffect(() => {
    async function load() {
      try {
        const vars = await listVariables();
        const user: Vars = {};
        for (const v of vars) {
          user[v.key] = v.value;
        }
        setUserVarsState(user);
      } catch {
        setUserVarsState({});
      }
    }
    load();
  }, []);

  const setUserVar = useCallback(async (name: string, value: string) => {
    await saveVariable(name, value);
    setUserVarsState((prev) => ({ ...prev, [name]: value }));
  }, []);

  const setUserVars = useCallback(async (vars: Vars) => {
    await Promise.all(Object.entries(vars).map(([k, v]) => saveVariable(k, v)));
    setUserVarsState((prev) => ({ ...prev, ...vars }));
  }, []);

  const removeUserVar = useCallback(async (name: string) => {
    await deleteVariable(name);
    setUserVarsState((prev) => {
      const { [name]: _, ...rest } = prev;
      return rest;
    });
  }, []);

  const clearUserVars = useCallback(async () => {
    await clearVariables();
    setUserVarsState({});
  }, []);

  const clearAutoVars = useCallback(() => {
    // no-op: auto vars are managed by server
    setAutoVarsState({});
  }, []);

  const substitute = useCallback(
    (template: string): string => {
      // Client-side preview only - actual resolution happens on server
      return template.replace(/\{\{(\w[\w.]*\w|\w)\}\}/g, (_match, name: string) => {
        return userVars[name] ?? autoVars[name] ?? `{{${name}}}`;
      });
    },
    [userVars, autoVars],
  );

  // Derived state
  const variables = useMemo(() => ({ ...autoVars, ...userVars }), [autoVars, userVars]);

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
      groupedAutoVars,
      userVarNames,
      allVarNames,
      varNames: allVarNames,
    }),
    [
      variables,
      userVars,
      autoVars,
      setUserVar,
      setUserVars,
      removeUserVar,
      clearUserVars,
      clearAutoVars,
      substitute,
      groupedAutoVars,
      userVarNames,
      allVarNames,
    ],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useVariables(): Ctx {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useVariables must be used within VariableProvider");
  return ctx;
}