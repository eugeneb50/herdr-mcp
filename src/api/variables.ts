export type Variable = {
  key: string;
  value: string;
  session_id?: string;
  execution_id?: string;
};

export type VariableResponse = {
  id: string;
  session_id?: string;
  execution_id?: string;
  key: string;
  value: unknown;
  created_at: string;
  updated_at: string;
};

export function variableDisplayValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (value === null || value === undefined) return "";
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

export async function listVariables(): Promise<Variable[]> {
  const res = await fetch("/api/variables");
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const data = await res.json();
  // Server returns VariableStore[] with id, key, value (any JSON), etc.
  // Stringify non-strings so the UI never renders `[object Object]`.
  return (data as VariableResponse[]).map((v) => ({ key: v.key, value: variableDisplayValue(v.value) }));
}

export async function saveVariable(key: string, value: unknown): Promise<void> {
  const res = await fetch("/api/variables", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ key, value }),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
}

export async function deleteVariable(key: string): Promise<void> {
  const res = await fetch(`/api/variables/${key}`, { method: "DELETE" });
  if (!res.ok && res.status !== 404) throw new Error(`HTTP ${res.status}`);
}

export async function clearVariables(): Promise<void> {
  const vars = await listVariables();
  await Promise.all(vars.map((v) => deleteVariable(v.key)));
}

export async function listAutoVariables(workspaceId?: string): Promise<Record<string, string>> {
  const qs = workspaceId ? `?workspace_id=${encodeURIComponent(workspaceId)}` : "";
  const res = await fetch(`/api/variables/auto${qs}`);
  if (!res.ok) return {};
  const data = await res.json();
  const vars = (data.variables ?? {}) as Record<string, unknown>;
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(vars)) {
    out[k] = variableDisplayValue(v);
  }
  return out;
}

export type PlaygroundSnapshot = {
  workspace_id: string;
  tool_name?: string | null;
  field_values: Record<string, string>;
  builder_recipe_name: string;
  builder_steps: unknown[];
};

export async function getPlayground(workspaceId = "default"): Promise<PlaygroundSnapshot | null> {
  try {
    const res = await fetch(`/api/playground?workspace_id=${encodeURIComponent(workspaceId)}`);
    if (!res.ok) return null;
    return (await res.json()) as PlaygroundSnapshot;
  } catch {
    return null;
  }
}

export async function savePlayground(snapshot: PlaygroundSnapshot): Promise<void> {
  try {
    await fetch("/api/playground", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(snapshot),
    });
  } catch {
    // best-effort: localStorage remains the fallback
  }
}