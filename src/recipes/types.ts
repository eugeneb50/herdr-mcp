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

export type TemplateVariableDef = {
  name: string;
  description: string;
  default_value?: unknown;
  required: boolean;
};

export type RecipeTemplateDef = {
  id: string;
  name: string;
  description: string;
  category: string;
  variables: TemplateVariableDef[];
  steps: RecipeStepDef[];
};
