export type { RecipeStepDef, RecipeDef, TemplateVariableDef, RecipeTemplateDef } from "./types";

export type { SavedRecipe, CreateRecipeInput, UpdateRecipeInput } from "../api/recipes";
export { listRecipes, getRecipe, saveRecipe, updateRecipe, deleteRecipe, duplicateRecipe, runRecipe, runRecipeById, listTemplates } from "../api/recipes";

export const PREDEFINED_RECIPES: import("./types").RecipeDef[] = [
  {
    name: "A2A Pane Inventory",
    description: "List panes + agents and store labels/ids via var_set. Fully runnable; check Variables after.",
    steps: [
      { id: "panes", tool: "list_panes", params: {}, description: "List panes (ids + labels)" },
      { id: "agents", tool: "agent_list", params: { workspace_id: "" }, description: "List registry agents" },
      { id: "store_panes", tool: "var_set", params: { session_id: "w1", key: "panes.snapshot", value: "{{ panes.result }}" }, description: "Store pane labels/ids" },
      { id: "store_agents", tool: "var_set", params: { session_id: "w1", key: "agents.snapshot", value: "{{ agents.result }}" }, description: "Store agent labels/ids" },
    ],
  },
  {
    name: "Explore Session",
    description: "Probe the current herdr session. Chain via {{ s3.result }} style step references.",
    steps: [
      { id: "s1", tool: "status", params: {}, description: "Get server status" },
      { id: "s2", tool: "list_workspaces", params: {}, description: "List workspaces" },
      { id: "s3", tool: "list_panes", params: {}, description: "List panes (ids + labels)" },
      { id: "s4", tool: "agent_list", params: { workspace_id: "" }, description: "List agents from registry" },
    ],
  },
  {
    name: "Create Dev Workspace",
    description: "Create a new workspace and inspect its initial state.",
    steps: [
      { id: "s1", tool: "create_workspace", params: { label: "dev" }, description: "Create workspace" },
      { id: "s2", tool: "list_panes", params: {}, description: "List panes in session" },
    ],
  },
  {
    name: "Split and Read",
    description: "Split a pane and read it. Replace <pane_id> with an id from s1 first.",
    steps: [
      { id: "s1", tool: "list_panes", params: {}, description: "Find panes" },
      { id: "s2", tool: "split_pane", params: { pane_id: "<pane_id>", direction: "right" }, description: "Split pane (edit id)" },
      { id: "s3", tool: "read_pane", params: { pane_id: "{{ s2.result }}", source: "visible" }, description: "Read new pane" },
    ],
  },
  {
    name: "Read → Wait → Read",
    description: "Read a pane, wait for output, read again. Replace <pane_id> with an id from s1.",
    steps: [
      { id: "s1", tool: "list_panes", params: {}, description: "Get pane list" },
      { id: "s2", tool: "read_pane", params: { pane_id: "<pane_id>", source: "visible" }, description: "Read initial visible output" },
      { id: "s3", tool: "wait_output", params: { pane_id: "<pane_id>", match_text: ".", use_regex: false, timeout_ms: 5000 }, description: "Wait for output" },
      { id: "s4", tool: "read_pane", params: { pane_id: "<pane_id>", source: "visible" }, description: "Read again" },
    ],
  },
  {
    name: "Start Agent in Pane",
    description: "Start a new agent via split. Picks up its pane via {{ s2.result }}.",
    steps: [
      { id: "s1", tool: "list_panes", params: {}, description: "Get pane list" },
      { id: "s2", tool: "split_pane", params: { pane_id: "<pane_id>", direction: "right" }, description: "Split pane (edit id)" },
      { id: "s3", tool: "start_agent", params: { name: "my-agent", split: "right", args: [] }, description: "Start agent in new pane" },
    ],
  },
  {
    name: "Goal-Driven Command",
    description: "Read a pane for context, then run a goal command. Replace <pane_id> first.",
    steps: [
      { id: "s1", tool: "list_panes", params: {}, description: "Get available panes" },
      { id: "s2", tool: "read_pane", params: { pane_id: "<pane_id>", source: "visible" }, description: "Read context" },
      { id: "s3", tool: "run_command", params: { pane_id: "<pane_id>", command: "/goal complete the instruction found in this file {{ s2.result }}" }, description: "Run goal command" },
    ],
  },
];
