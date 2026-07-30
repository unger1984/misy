/** Dynamic Kimi model discovery with a bundled outage fallback. */

import { endpointUrl, fetchWithTimeout } from "@misy/provider-sdk";
import type { ProviderConfig } from "./config";
import type { KimiHeaders } from "./headers";
import { type Credentials, isRecord, type Model } from "./types";

/** Remote request protocol selected by Kimi's model catalog. */
export type KimiProtocol = "openai" | "anthropic";

/** Normalized public models plus provider-local wire routing metadata. */
export type ModelCatalog = {
	models: Model[];
	protocols: ReadonlyMap<string, KimiProtocol>;
	authoritative: boolean;
};

type CatalogEntry = Model & { protocol: KimiProtocol };

/** HTTP failure from strict model discovery used to route a chat request. */
export class ModelCatalogRequestError extends Error {
	/** Creates a status-bearing catalog failure without including response credentials or bodies. */
	constructor(readonly status: number) {
		super(`Kimi models request failed (${status})`);
	}
}

/** The provider-local default used when Kimi cannot publish a catalog. */
export const DEFAULT_MODEL_ID = "kimi-for-coding";

const FALLBACK_MODELS: readonly CatalogEntry[] = [
	imageModel({
		id: "kimi-for-coding",
		display_name: "K2.7 Coding",
		context_window: 262_144,
		protocol: "anthropic",
	}),
	imageModel({
		id: "kimi-for-coding-highspeed",
		display_name: "K2.7 Coding Highspeed",
		context_window: 262_144,
		protocol: "anthropic",
	}),
	imageModel({
		id: "k3",
		display_name: "K3",
		context_window: 1_048_576,
		protocol: "openai",
		thinking: k3Thinking(),
	}),
	imageModel({
		id: "k3-256k",
		display_name: "K3 256K",
		context_window: 262_144,
		protocol: "openai",
		thinking: k3Thinking(),
	}),
];

/** Lists Kimi's live catalog and provider-local protocols, retaining an outage fallback. */
export async function listModelCatalog(
	config: ProviderConfig,
	headers: KimiHeaders,
	credentials: Credentials | undefined,
): Promise<ModelCatalog> {
	if (!credentials?.access_token) return fallbackCatalog();
	try {
		return await fetchModelCatalog(config, headers, credentials);
	} catch {
		return fallbackCatalog();
	}
}

/** Resolves one live model's request protocol without applying outage fallback semantics. */
export async function discoverModelProtocol(
	config: ProviderConfig,
	headers: KimiHeaders,
	credentials: Credentials,
	modelId: string,
	signal?: AbortSignal,
): Promise<KimiProtocol | undefined> {
	const modelCatalog = await fetchModelCatalog(config, headers, credentials, signal);
	return modelCatalog.protocols.get(modelId);
}

/** Returns copies of bundled models so callers cannot mutate the fallback source. */
export function fallbackModels(): Model[] {
	return fallbackCatalog().models;
}

/** Returns bundled models and their provider-local request protocols. */
export function fallbackCatalog(): ModelCatalog {
	return catalog(FALLBACK_MODELS, false);
}

function parseModels(value: unknown): CatalogEntry[] {
	if (!isRecord(value) || !Array.isArray(value["data"])) {
		throw new Error("Kimi models response did not contain data");
	}
	const models = value["data"]
		.map(parseModel)
		.filter((model): model is CatalogEntry => model !== undefined);
	if (models.length === 0) throw new Error("Kimi models response did not contain valid models");
	return models;
}

async function fetchModelCatalog(
	config: ProviderConfig,
	headers: KimiHeaders,
	credentials: Credentials,
	signal?: AbortSignal,
): Promise<ModelCatalog> {
	const response = await fetchWithTimeout(
		endpointUrl(config.apiBaseUrl, "models"),
		{
			signal,
			headers: {
				...headers.common(),
				authorization: `Bearer ${credentials.access_token}`,
			},
		},
		config.requestTimeoutMs,
	);
	if (!response.ok) throw new ModelCatalogRequestError(response.status);
	return catalog(parseModels(await response.json()), true);
}

function parseModel(value: unknown): CatalogEntry | undefined {
	if (!isRecord(value) || typeof value["id"] !== "string" || value["id"].trim() === "")
		return undefined;
	const id = value["id"];
	const protocol = modelProtocol(value["protocol"]);
	const thinking = thinkingMetadata(id, protocol);
	return {
		id,
		display_name:
			typeof value["display_name"] === "string" && value["display_name"].trim()
				? value["display_name"]
				: value["id"],
		context_window:
			typeof value["context_length"] === "number" && value["context_length"] > 0
				? value["context_length"]
				: 262_144,
		input_modalities:
			value["supports_image_in"] === true || id.toLowerCase().startsWith("kimi-k2")
				? ["text", "image"]
				: ["text"],
		protocol,
		...(typeof value["description"] === "string" && value["description"].trim()
			? { description: value["description"] }
			: {}),
		...(typeof value["pricing"] === "string" && value["pricing"].trim()
			? { pricing: value["pricing"] }
			: {}),
		...(thinking === undefined ? {} : { thinking }),
	};
}

function thinkingMetadata(id: string, protocol: KimiProtocol): Model["thinking"] {
	if (protocol !== "openai" || (id !== "k3" && id !== "k3-256k")) return undefined;
	return k3Thinking();
}

function k3Thinking(): NonNullable<Model["thinking"]> {
	return {
		default: "high",
		levels: [
			{ id: "low", description: "Lower reasoning effort" },
			{ id: "high", description: "Recommended reasoning effort" },
			{ id: "max", description: "Maximum reasoning effort" },
		],
	};
}

function modelProtocol(value: unknown): KimiProtocol {
	// Kimi introduced `null` for OpenAI after legacy catalogs had already omitted
	// the field; absence must therefore retain the old Anthropic-compatible route.
	return value === null ? "openai" : "anthropic";
}

function imageModel(model: Omit<CatalogEntry, "input_modalities">): CatalogEntry {
	return { ...model, input_modalities: ["text", "image"] };
}

function catalog(entries: readonly CatalogEntry[], authoritative: boolean): ModelCatalog {
	return {
		models: entries.map(({ protocol: _protocol, ...model }) => ({
			...model,
			input_modalities: [...model.input_modalities],
			...(model.thinking
				? {
						thinking: {
							...model.thinking,
							levels: model.thinking.levels.map((level) => ({ ...level })),
						},
					}
				: {}),
		})),
		protocols: new Map(entries.map((entry) => [entry.id, entry.protocol])),
		authoritative,
	};
}
