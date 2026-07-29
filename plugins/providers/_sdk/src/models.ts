/**
 * Shared default-model selection for `models.list` results.
 *
 * The core falls back to the first listed model only when a provider omits `default_model`; it
 * cannot know a provider's preferred default. Providers therefore declare the preference here,
 * once, instead of re-implementing the selection in every plugin.
 */

/**
 * Returns the provider-preferred default when the catalog lists it, otherwise the first model.
 *
 * Returns undefined for an empty catalog, leaving the `default_model` field out of the result.
 */
export function preferredDefaultModel(
	models: readonly { id: string }[],
	preferredId: string,
): string | undefined {
	if (models.some((model) => model.id === preferredId)) return preferredId;
	return models[0]?.id;
}
