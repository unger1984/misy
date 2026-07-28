/** Dynamic Anthropic model discovery with an offline bundled fallback catalog. */
import { endpointUrl, fetchWithTimeout, preferredDefaultModel } from "@misy/provider-sdk";
import type { ProviderConfig } from "./config";
import { type Credentials, isRecord, type Json } from "./types";

const DEFAULT_CONTEXT_WINDOW = 200_000;

/** A model entry returned by `models.list`. */
export type Model = {
	id: string;
	display_name: string;
	context_window: number;
};

const BUNDLED_MODELS: readonly Model[] = [
	{ id: "claude-opus-4-8", display_name: "Claude Opus 4.8", context_window: 1_000_000 },
	{ id: "claude-sonnet-4-6", display_name: "Claude Sonnet 4.6", context_window: 1_000_000 },
	{ id: "claude-haiku-4-5-20251001", display_name: "Claude Haiku 4.5", context_window: 200_000 },
];

/** The preferred bundled model when it is available to the signed-in account. */
export const DEFAULT_MODEL_ID = "claude-opus-4-8";

/** Fetches models from Anthropic, falling back to the bundled catalog on any endpoint failure. */
export async function listModels(
	config: ProviderConfig,
	credentials: Credentials,
): Promise<Model[]> {
	try {
		const response = await fetchWithTimeout(
			endpointUrl(config.apiBaseUrl, "v1/models"),
			{
				method: "GET",
				headers: discoveryHeaders(credentials),
			},
			config.requestTimeoutMs,
		);
		if (!response.ok) throw new Error(`Anthropic model listing failed (${response.status})`);
		return parseModels(await response.json());
	} catch {
		return bundledModels();
	}
}

/** Chooses a declared default only when that model is in the dynamic catalog. */
export function defaultModel(models: readonly Model[]): string | undefined {
	return preferredDefaultModel(models, DEFAULT_MODEL_ID);
}

function discoveryHeaders(credentials: Credentials): Record<string, string> {
	return {
		authorization: `Bearer ${credentials["access_token"]}`,
		"anthropic-version": "2023-06-01",
		"anthropic-beta": "oauth-2025-04-20",
		"anthropic-dangerous-direct-browser-access": "true",
	};
}

function parseModels(value: unknown): Model[] {
	if (!isRecord(value) || !Array.isArray(value["data"])) {
		throw new Error("Anthropic model listing did not include a data array");
	}
	const models = value["data"].map(parseModel);
	if (models.length === 0) throw new Error("Anthropic model listing returned no valid models");
	return models;
}

function parseModel(value: Json): Model {
	if (!isRecord(value) || typeof value["id"] !== "string" || value["id"].length === 0) {
		throw new Error("Anthropic model listing contains an item without a string id");
	}
	const id = value["id"];
	const displayName = value["display_name"];
	return {
		id,
		display_name: typeof displayName === "string" && displayName.length > 0 ? displayName : id,
		context_window: contextWindow(id),
	};
}

function contextWindow(id: string): number {
	return BUNDLED_MODELS.find((model) => model.id === id)?.context_window ?? DEFAULT_CONTEXT_WINDOW;
}

function bundledModels(): Model[] {
	return BUNDLED_MODELS.map((model) => ({ ...model }));
}
