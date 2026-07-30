/** Display-only reference metadata derived from the models.dev catalog snapshot. */

type Price = readonly [input: number, output: number];

const PRICES: ReadonlyArray<readonly [pattern: RegExp, price: Price]> = [
	[/gpt-5[.-]6-sol/, [5, 30]],
	[/gpt-5[.-]6-terra/, [2.5, 15]],
	[/gpt-5[.-]6-luna/, [1, 6]],
	[/gpt-5[.-]5/, [5, 30]],
	[/gpt-5[.-]4-mini/, [0.75, 4.5]],
	[/gpt-5[.-]4/, [2.5, 15]],
	[/gpt-5[.-]3-codex-spark/, [1.75, 14]],
	[/gemini-3[.-]5-flash-lite/, [0.3, 2.5]],
	[/gemini-3[.-]5-flash/, [1.5, 9]],
	[/gemini-3[.-]1-pro/, [2, 12]],
	[/gemini-3[.-]1-flash-lite/, [0.25, 1.5]],
	[/gemini-3-flash/, [0.5, 3]],
	[/gemini-2[.-]5-pro/, [1.25, 10]],
	[/gemini-2[.-]5-flash-lite/, [0.1, 0.4]],
	[/gemini-2[.-]5-flash/, [0.3, 2.5]],
	[/glm-5[.-]1/, [1.4, 4.4]],
	[/glm-5(?:$|-)/, [1, 3.2]],
];

/**
 * Returns a compact `$input/output` USD-per-million reference when upstream omits pricing.
 *
 * Provider-supplied pricing always takes precedence at the call site.
 */
export function referencePricing(modelSelector: string): string | undefined {
	const selector = modelSelector.toLowerCase();
	if (selector === "free" || selector.endsWith("/free")) return "free";
	for (const [pattern, [input, output]] of PRICES) {
		if (pattern.test(selector)) return `$${input}/${output}`;
	}
	return undefined;
}
