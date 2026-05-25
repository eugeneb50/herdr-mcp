export type RecipeStepDef = {
  id: string;
  tool: string;
  params: Record<string, unknown>;
  description?: string;
};

export type RecipeDef = {
  name: string;
  description: string;
  steps: RecipeStepDef[];
};

export type { SavedRecipe, RecipeInput } from "./recipeDb";
export { listRecipes, getRecipe, saveRecipe, updateRecipe, deleteRecipe, duplicateRecipe } from "./recipeDb";

export const PREDEFINED_RECIPES: RecipeDef[] = [
  {
    name: "Explore Session",
    description: "Probe the current herdr session and populate variables (pane_id_1, workspace_id_1, etc.).",
    steps: [
      { id: "s1", tool: "status", params: {}, description: "Get server status" },
      { id: "s2", tool: "list_workspaces", params: {}, description: "List workspaces → populates workspace_id_1..N" },
      { id: "s3", tool: "list_panes", params: {}, description: "List panes → populates pane_id_1..N" },
      { id: "s4", tool: "list_agents", params: {}, description: "List agents → populates target_1..N" },
    ],
  },
  {
    name: "Create Dev Workspace",
    description: "Create a new workspace and inspect its initial state. Uses {{workspace_id}} from create_workspace.",
    steps: [
      { id: "s1", tool: "create_workspace", params: { label: "dev" }, description: "Create workspace → populates workspace_id" },
      { id: "s2", tool: "list_panes", params: {}, description: "List panes in session" },
    ],
  },
  {
    name: "Split and Read",
    description: "Split the first pane and read its output. Uses {{pane_id_1}} from list_panes.",
    steps: [
      { id: "s1", tool: "list_panes", params: {}, description: "Find panes → populates pane_id_1" },
      { id: "s2", tool: "split_pane", params: { pane_id: "{{pane_id_1}}", direction: "right" }, description: "Split first pane → populates pane_id" },
      { id: "s3", tool: "read_pane", params: { pane_id: "{{pane_id}}", source: "visible" }, description: "Read new pane → populates pane_output" },
    ],
  },
  {
    name: "Read → Wait → Read",
    description: "Read a pane, wait for new output, then read again. Uses {{pane_id_1}} from list_panes.",
    steps: [
      { id: "s1", tool: "list_panes", params: {}, description: "Get pane list → populates pane_id_1" },
      { id: "s2", tool: "read_pane", params: { pane_id: "{{pane_id_1}}", source: "visible" }, description: "Read initial visible output" },
      { id: "s3", tool: "wait_output", params: { pane_id: "{{pane_id_1}}", match_text: ".", use_regex: false, timeout_ms: 5000 }, description: "Wait for output" },
      { id: "s4", tool: "read_pane", params: { pane_id: "{{pane_id_1}}", source: "visible" }, description: "Read again" },
    ],
  },
  {
    name: "Start Agent in Pane",
    description: "Split a pane and start a new agent in it. Uses {{pane_id_1}} from list_panes.",
    steps: [
      { id: "s1", tool: "list_panes", params: {}, description: "Get pane list → populates pane_id_1" },
      { id: "s2", tool: "split_pane", params: { pane_id: "{{pane_id_1}}", direction: "right" }, description: "Split pane → populates pane_id" },
      { id: "s3", tool: "start_agent", params: { name: "my-agent", split: "right" }, description: "Start agent in new pane" },
    ],
  },
  {
    name: "Goal-Driven Command",
    description: "Find a pane, read context to discover a file path, then run a goal command with it.",
    steps: [
      { id: "s1", tool: "list_panes", params: {}, description: "Get available panes → populates pane_id_1" },
      { id: "s2", tool: "read_pane", params: { pane_id: "{{pane_id_1}}", source: "visible" }, description: "Read context → populates pane_output and file_path_1" },
      { id: "s3", tool: "run_command", params: { pane_id: "{{pane_id_1}}", command: "/goal complete the instruction found in this file {{file_path_1}}" }, description: "Run goal command" },
    ],
  },
];
