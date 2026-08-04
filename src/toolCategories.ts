export type ToolCategory = {
  key: string;
  label: string;
  accent: string;
  tools: string[];
};

export const TOOL_CATEGORIES: ToolCategory[] = [
  { key: "Discovery", label: "Discovery", accent: "sky", tools: ["status", "list_workspaces", "list_tabs", "list_panes", "list_agents", "get_pane", "get_agent"] },
  { key: "Lifecycle", label: "Lifecycle", accent: "emerald", tools: ["create_workspace", "create_tab", "split_pane", "close_pane", "start_agent"] },
  { key: "Read", label: "Read", accent: "teal", tools: ["read_pane", "read_agent"] },
  { key: "Write", label: "Write", accent: "amber", tools: ["send_text", "send_keys", "run_command", "send_agent"] },
  { key: "Synchronize", label: "Synchronize", accent: "rose", tools: ["wait_output", "wait_pane_agent_status", "wait_agent_status"] },
  { key: "A2A", label: "A2A", accent: "violet", tools: ["agent_spawn", "agent_message", "agent_read", "agent_wait", "agent_list"] },
  { key: "Variables", label: "Variables", accent: "orange", tools: ["var_get", "var_set"] },
  { key: "Templates", label: "Templates", accent: "cyan", tools: ["list_templates", "get_template", "instantiate_template"] },
  { key: "Trim", label: "Trim", accent: "pink", tools: ["compress", "decompress", "trim_policy_set", "trim_policy_get", "trim_eval", "trim_bench", "trim_status", "trim_diagnose", "trim_summary", "trim_dashboard_open"] },
  { key: "Scheduler", label: "Scheduler", accent: "lime", tools: ["schedule_recipe", "list_schedules", "delete_schedule", "enable_schedule"] },
  { key: "Folder Key", label: "Folder Key", accent: "yellow", tools: ["build_folder_key", "get_folder_key", "list_folder_keys", "decompress_with_folder_key"] },
  { key: "Clipboard", label: "Clipboard", accent: "stone", tools: ["clipboard_set", "clipboard_get"] },
];

export const TOOL_CATEGORY_MAP: Record<string, string> = {};
for (const cat of TOOL_CATEGORIES) {
  for (const tool of cat.tools) {
    TOOL_CATEGORY_MAP[tool] = cat.key;
  }
}

export function getCategoryForTool(toolName: string): string {
  return TOOL_CATEGORY_MAP[toolName] ?? "Other";
}

export function groupToolsByCategory(toolNames: string[]): { key: string; label: string; tools: string[] }[] {
  const grouped: Record<string, string[]> = {};
  const catOrder: string[] = [];

  for (const name of toolNames) {
    const cat = getCategoryForTool(name);
    if (!grouped[cat]) {
      grouped[cat] = [];
      catOrder.push(cat);
    }
    grouped[cat].push(name);
  }

  return catOrder.map((key) => {
    const cat = TOOL_CATEGORIES.find((c) => c.key === key);
    return { key, label: cat?.label ?? key, tools: grouped[key].sort() };
  });
}
