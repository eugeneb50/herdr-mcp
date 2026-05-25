import type { RecipeDef, RecipeStepDef } from "./index";

export type SavedRecipe = RecipeDef & {
  id: string;
  createdAt: number;
  updatedAt: number;
};

export type RecipeInput = {
  name: string;
  description?: string;
  steps: RecipeStepDef[];
};

const DB_NAME = "herdr_mcp_recipes";
const DB_VERSION = 1;
const STORE_NAME = "recipes";

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        const store = db.createObjectStore(STORE_NAME, { keyPath: "id" });
        store.createIndex("name", "name", { unique: false });
        store.createIndex("updatedAt", "updatedAt", { unique: false });
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function generateId(): string {
  const ts = Date.now().toString(36);
  const rand = Math.random().toString(36).slice(2, 6);
  return `rec_${ts}_${rand}`;
}

function storeOp<T>(fn: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  return new Promise(async (resolve, reject) => {
    try {
      const db = await openDb();
      const tx = db.transaction(STORE_NAME, "readwrite");
      const store = tx.objectStore(STORE_NAME);
      const req = fn(store);
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
      tx.oncomplete = () => db.close();
    } catch (e) {
      reject(e);
    }
  });
}

function readOp<T>(fn: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  return new Promise(async (resolve, reject) => {
    try {
      const db = await openDb();
      const tx = db.transaction(STORE_NAME, "readonly");
      const store = tx.objectStore(STORE_NAME);
      const req = fn(store);
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
      tx.oncomplete = () => db.close();
    } catch (e) {
      reject(e);
    }
  });
}

export async function listRecipes(): Promise<SavedRecipe[]> {
  return new Promise(async (resolve, reject) => {
    try {
      const db = await openDb();
      const tx = db.transaction(STORE_NAME, "readonly");
      const store = tx.objectStore(STORE_NAME);
      const index = store.index("updatedAt");
      const req = index.openCursor(null, "prev");
      const results: SavedRecipe[] = [];
      req.onsuccess = () => {
        const cursor = req.result;
        if (cursor) {
          results.push(cursor.value);
          cursor.continue();
        } else {
          resolve(results);
        }
      };
      req.onerror = () => reject(req.error);
      tx.oncomplete = () => db.close();
    } catch (e) {
      reject(e);
    }
  });
}

export async function getRecipe(id: string): Promise<SavedRecipe | undefined> {
  return readOp((store) => store.get(id));
}

export async function saveRecipe(input: RecipeInput): Promise<string> {
  const id = generateId();
  const now = Date.now();
  const recipe: SavedRecipe = {
    id,
    name: input.name,
    description: input.description ?? `${input.steps.length} step(s)`,
    steps: input.steps,
    createdAt: now,
    updatedAt: now,
  };
  await storeOp((store) => store.add(recipe));
  return id;
}

export async function updateRecipe(
  id: string,
  input: Partial<RecipeInput>,
): Promise<void> {
  const existing = await getRecipe(id);
  if (!existing) throw new Error(`Recipe ${id} not found`);
  const updated: SavedRecipe = {
    ...existing,
    name: input.name ?? existing.name,
    description: input.description ?? existing.description,
    steps: input.steps ?? existing.steps,
    updatedAt: Date.now(),
  };
  await storeOp((store) => store.put(updated));
}

export async function deleteRecipe(id: string): Promise<void> {
  await storeOp((store) => store.delete(id));
}

export async function duplicateRecipe(id: string): Promise<string> {
  const existing = await getRecipe(id);
  if (!existing) throw new Error(`Recipe ${id} not found`);
  const newId = generateId();
  const now = Date.now();
  const copy: SavedRecipe = {
    ...existing,
    id: newId,
    name: `Copy of ${existing.name}`,
    createdAt: now,
    updatedAt: now,
  };
  await storeOp((store) => store.add(copy));
  return newId;
}
