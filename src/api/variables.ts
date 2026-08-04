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
  value: string;
  created_at: string;
  updated_at: string;
};

export async function listVariables(): Promise<Variable[]> {
  const res = await fetch("/api/variables");
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const data = await res.json();
  // Server returns VariableStore[] with id, key, value, etc.
  return (data as VariableResponse[]).map((v) => ({ key: v.key, value: v.value }));
}

export async function saveVariable(key: string, value: string): Promise<void> {
  const res = await fetch("/api/variables", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ key, value: value }),
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