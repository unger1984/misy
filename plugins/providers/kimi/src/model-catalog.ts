/** Dynamic Kimi model discovery with a bundled outage fallback. */

import { endpointUrl, fetchWithTimeout } from "@misy/provider-sdk";
import type { ProviderConfig } from "./config";
import type { KimiHeaders } from "./headers";
import { type Credentials, isRecord, type Model } from "./types";

/** The provider-local default used when Kimi cannot publish a catalog. */
export const DEFAULT_MODEL_ID = "kimi-for-coding";

const FALLBACK_MODELS: readonly Model[] = [
	{ id: "kimi-for-coding", display_name: "K2.7 Coding", context_window: 262_144 },
	{
		id: "kimi-for-coding-highspeed",
		display_name: "K2.7 Coding Highspeed",
		context_window: 262_144,
	},
	{ id: "k3", display_name: "K3", context_window: 1_048_576 },
];

/** Lists Kimi's live catalog, retaining the bundled catalog for endpoint failures. */
export async function listModels(
	config: ProviderConfig,
	headers: KimiHeaders,
	credentials: Credentials | undefined,
): Promise<Model[]> {
	if (!credentials?.access_token) return fallbackModels();
	try {
		const response = await fetchWithTimeout(
			endpointUrl(config.apiBaseUrl, "models"),
			{
				headers: {
					...headers.common(),
					authorization: `Bearer ${credentials.access_token}`,
				},
			},
			config.requestTimeoutMs,
		);
		if (!response.ok) throw new Error(`Kimi models request failed (${response.status})`);
		return parseModels(await response.json());
	} catch {
		return fallbackModels();
	}
}

/** Returns copies of bundled models so callers cannot mutate the fallback source. */
export function fallbackModels(): Model[] {
	return FALLBACK_MODELS.map((model) => ({ ...model }));
}

function parseModels(value: unknown): Model[] {
	if (!isRecord(value) || !Array.isArray(value["data"])) {
		throw new Error("Kimi models response did not contain data");
	}
	const models = value["data"]
		.map(parseModel)
		.filter((model): model is Model => model !== undefined);
	if (models.length === 0) throw new Error("Kimi models response did not contain valid models");
	return models;
}

function parseModel(value: unknown): Model | undefined {
	if (!isRecord(value) || typeof value["id"] !== "string" || value["id"].trim() === "")
		return undefined;
	return {
		id: value["id"],
		display_name:
			typeof value["display_name"] === "string" && value["display_name"].trim()
				? value["display_name"]
				: value["id"],
		context_window:
			typeof value["context_length"] === "number" && value["context_length"] > 0
				? value["context_length"]
				: 262_144,
	};
}
