import type { RecipeStepDef, RecipeDef, RecipeTemplateDef } from "../recipes/types";

export type { RecipeStepDef, RecipeDef, RecipeTemplateDef } from "../recipes/types";

export type SavedRecipe = RecipeDef & {
  id: string;
  createdAt: number;
  updatedAt: number;
};

export type RecipeResponse = {
  status: string;
  results: Record<string, unknown>;
  variables?: Record<string, unknown>;
};

export type CreateRecipeInput = {
  name: string;
  description?: string;
  steps: RecipeStepDef[];
};

export type UpdateRecipeInput = {
  name?: string;
  description?: string;
  steps?: RecipeStepDef[];
};

async function handleResponse<T>(res: Response): Promise<T> {
  const text = await res.text();
  let data: unknown;
  try { data = JSON.parse(text); } catch { data = { raw: text }; }
  if (!res.ok) {
    const msg = (data as Record<string, unknown>)?.error as string ?? `HTTP ${res.status}`;
    throw new Error(msg);
  }
  return data as T;
}

export async function listRecipes(): Promise<SavedRecipe[]> {
  const res = await fetch("/api/recipes");
  return handleResponse<SavedRecipe[]>(res);
}

export async function getRecipe(id: string): Promise<SavedRecipe> {
  const res = await fetch(`/api/recipes/${id}`);
  return handleResponse<SavedRecipe>(res);
}

export async function saveRecipe(input: CreateRecipeInput): Promise<SavedRecipe> {
  const res = await fetch("/api/recipes", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
  return handleResponse<SavedRecipe>(res);
}

export async function updateRecipe(id: string, input: UpdateRecipeInput): Promise<SavedRecipe> {
  const res = await fetch(`/api/recipes/${id}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
  return handleResponse<SavedRecipe>(res);
}

export async function deleteRecipe(id: string): Promise<void> {
  const res = await fetch(`/api/recipes/${id}`, { method: "DELETE" });
  await handleResponse<void>(res);
}

export async function duplicateRecipe(id: string): Promise<SavedRecipe> {
  const recipe = await getRecipe(id);
  const { id: _, createdAt, updatedAt, ...rest } = recipe;
  return saveRecipe({
    ...rest,
    name: `Copy of ${recipe.name}`,
  });
}

export async function runRecipeById(id: string): Promise<RecipeResponse> {
  const res = await fetch(`/api/recipes/${id}/run`, { method: "POST" });
  return handleResponse<RecipeResponse>(res);
}

export async function runRecipe(steps: RecipeStepDef[], name?: string): Promise<RecipeResponse> {
  const res = await fetch("/api/recipe", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, steps }),
  });
  return handleResponse<RecipeResponse>(res);
}

export async function listTemplates(): Promise<RecipeTemplateDef[]> {
  const res = await fetch("/api/templates");
  return handleResponse<RecipeTemplateDef[]>(res);
}
