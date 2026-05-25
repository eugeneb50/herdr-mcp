import { useCallback, useEffect, useState } from "react";
import { PREDEFINED_RECIPES, listRecipes, deleteRecipe, duplicateRecipe } from "../recipes";
import type { RecipeDef, SavedRecipe } from "../recipes";

type Props = {
  onSelect: (recipe: RecipeDef) => void;
  refreshKey?: number;
};

export function RecipeLibrary({ onSelect, refreshKey = 0 }: Props) {
  const [saved, setSaved] = useState<SavedRecipe[]>([]);

  const load = useCallback(async () => {
    try {
      const items = await listRecipes();
      setSaved(items);
    } catch { /* indexedDB may be unavailable */ }
  }, []);

  useEffect(() => {
    load();
  }, [refreshKey, load]);

  const handleDelete = useCallback(async (id: string) => {
    try {
      await deleteRecipe(id);
      setSaved((prev) => prev.filter((r) => r.id !== id));
    } catch { /* ignore */ }
  }, []);

  const handleDuplicate = useCallback(async (id: string) => {
    try {
      await duplicateRecipe(id);
      load();
    } catch { /* ignore */ }
  }, [load]);

  return (
    <div>
      <h3 className="text-xs font-mono uppercase tracking-wider text-neutral-500 mb-3">Recipe Library</h3>

      {PREDEFINED_RECIPES.length > 0 && (
        <div className="mb-4">
          <div className="text-[10px] font-mono uppercase tracking-wider text-neutral-600 mb-2 px-1">Templates</div>
          <div className="space-y-2">
            {PREDEFINED_RECIPES.map((recipe) => (
              <button
                key={recipe.name}
                onClick={() => onSelect(recipe)}
                className="w-full text-left p-3 rounded-lg border border-neutral-800 bg-neutral-900/40 hover:bg-neutral-800/60 hover:border-neutral-700 transition-colors"
              >
                <div className="text-sm font-medium text-neutral-200">{recipe.name}</div>
                <div className="text-[11px] text-neutral-500 mt-0.5 line-clamp-2">{recipe.description}</div>
                <div className="text-[10px] text-neutral-600 font-mono mt-1.5">{recipe.steps.length} step(s)</div>
              </button>
            ))}
          </div>
        </div>
      )}

      <div>
        <div className="text-[10px] font-mono uppercase tracking-wider text-neutral-600 mb-2 px-1">
          My Recipes {saved.length > 0 && <span className="text-neutral-500">({saved.length})</span>}
        </div>
        {saved.length === 0 && (
          <p className="text-[11px] text-neutral-500 italic px-1">No saved recipes yet. Build one and click Save.</p>
        )}
        <div className="space-y-2">
          {saved.map((recipe) => (
            <div
              key={recipe.id}
              className="group relative rounded-lg border border-neutral-800 bg-neutral-900/40 hover:bg-neutral-800/60 hover:border-neutral-700 transition-colors"
            >
              <button
                onClick={() => onSelect(recipe)}
                className="w-full text-left p-3 pr-16"
              >
                <div className="text-sm font-medium text-neutral-200">{recipe.name}</div>
                <div className="text-[11px] text-neutral-500 mt-0.5 line-clamp-2">{recipe.description}</div>
                <div className="text-[10px] text-neutral-600 font-mono mt-1.5">
                  {recipe.steps.length} step(s) &middot; {new Date(recipe.updatedAt).toLocaleDateString()}
                </div>
              </button>
              <div className="absolute right-2 top-2 flex gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                <button
                  onClick={(e) => { e.stopPropagation(); handleDuplicate(recipe.id); }}
                  className="p-1.5 rounded bg-neutral-800 text-neutral-400 hover:text-emerald-300 hover:bg-neutral-700 transition-colors"
                  title="Duplicate"
                >
                  <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2">
                    <path strokeLinecap="round" strokeLinejoin="round" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
                  </svg>
                </button>
                <button
                  onClick={(e) => { e.stopPropagation(); handleDelete(recipe.id); }}
                  className="p-1.5 rounded bg-neutral-800 text-neutral-400 hover:text-rose-400 hover:bg-neutral-700 transition-colors"
                  title="Delete"
                >
                  <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2">
                    <path strokeLinecap="round" strokeLinejoin="round" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                  </svg>
                </button>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
