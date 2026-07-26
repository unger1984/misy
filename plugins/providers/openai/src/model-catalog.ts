/** Bundled ChatGPT subscription model catalog; listing must not call an undocumented endpoint. */

/** A model the OpenAI provider can expose through `models.list`. */
export type Model = {
	id: string;
	display_name: string;
	context_window: number;
	reasoning: boolean;
};

/** The ChatGPT Codex default from OMP's `provider-models/descriptors.ts`. */
export const DEFAULT_MODEL_ID = "gpt-5.5";

// Source: `.references/oh-my-pi/packages/catalog/src/models.json` → `openai-codex`.
// Recheck this deliberately static list whenever the bundled OMP reference is updated.
const MODELS: readonly Model[] = [
	{ id: "gpt-5.6-terra", display_name: "GPT-5.6 Terra", context_window: 372_000, reasoning: true },
	{ id: "gpt-5.6-sol", display_name: "GPT-5.6 Sol", context_window: 372_000, reasoning: true },
	{ id: "gpt-5.6-luna", display_name: "GPT-5.6 Luna", context_window: 372_000, reasoning: true },
	{ id: "gpt-5.5", display_name: "GPT-5.5", context_window: 272_000, reasoning: true },
	{ id: "gpt-5.4", display_name: "GPT-5.4", context_window: 272_000, reasoning: true },
	{ id: "gpt-5.4-mini", display_name: "GPT-5.4 mini", context_window: 272_000, reasoning: true },
	{
		id: "gpt-5.3-codex-spark",
		display_name: "GPT-5.3 Codex Spark",
		context_window: 128_000,
		reasoning: true,
	},
];

/** Returns the static catalog as fresh objects so callers cannot mutate it. */
export function listModels(): Model[] {
	return MODELS.map((model) => ({ ...model }));
}

/** Returns whether a catalog model supports the Responses reasoning controls. */
export function supportsReasoning(modelId: string): boolean {
	return MODELS.some((model) => model.id === modelId && model.reasoning);
}
