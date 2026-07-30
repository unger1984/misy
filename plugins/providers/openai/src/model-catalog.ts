/** Discovers account-scoped Codex models and retains a bundled offline fallback. */

import { fetchWithTimeout } from "@misy/provider-sdk";
import { authHeaders } from "./auth";
import type { ProviderConfig } from "./config";
import type { Credentials } from "./types";

const DEFAULT_CONTEXT_WINDOW = 272_000;
const GPT_5_6_CONTEXT_WINDOW = 372_000;

/** A model the OpenAI provider can expose through `models.list`. */
export type Model = {
	id: string;
	display_name: string;
	context_window: number;
	reasoning: boolean;
	description?: string;
	pricing?: string;
	thinking?: {
		default: string;
		levels: { id: string; description: string }[];
	};
	input_modalities: ("text" | "image")[];
};

type CatalogEntry = Model & { priority: number };

/** The ChatGPT Codex default from OMP's `provider-models/descriptors.ts`. */
export const DEFAULT_MODEL_ID = "gpt-5.5";

// This list keeps model selection usable during offline discovery failures.
const FALLBACK_MODELS: readonly Model[] = [
	imageModel({
		id: "gpt-5.6-terra",
		display_name: "GPT-5.6 Terra",
		context_window: 372_000,
		reasoning: true,
	}),
	imageModel({
		id: "gpt-5.6-sol",
		display_name: "GPT-5.6 Sol",
		context_window: 372_000,
		reasoning: true,
	}),
	imageModel({
		id: "gpt-5.6-luna",
		display_name: "GPT-5.6 Luna",
		context_window: 372_000,
		reasoning: true,
	}),
	imageModel({ id: "gpt-5.5", display_name: "GPT-5.5", context_window: 272_000, reasoning: true }),
	imageModel({ id: "gpt-5.4", display_name: "GPT-5.4", context_window: 272_000, reasoning: true }),
	imageModel({
		id: "gpt-5.4-mini",
		display_name: "GPT-5.4 mini",
		context_window: 272_000,
		reasoning: true,
	}),
	imageModel({
		id: "gpt-5.3-codex-spark",
		display_name: "GPT-5.3 Codex Spark",
		context_window: 128_000,
		reasoning: true,
	}),
];

let reasoningByModel = reasoningLookup(FALLBACK_MODELS);
let lastCatalogSource: "remote" | "bundled" = "bundled";

/** Returns the source of the most recently resolved catalog. */
export function catalogSource(): "remote" | "bundled" {
	return lastCatalogSource;
}

/**
 * Lists models available to the supplied subscription, using bundled models when discovery fails.
 *
 * Network, HTTP-status, and malformed-response failures are deliberately converted to the offline
 * catalog because model selection must remain available while ChatGPT's private discovery API is
 * temporarily unavailable.
 */
export async function listModels(
	config: ProviderConfig,
	credentials: Credentials,
): Promise<Model[]> {
	for (const path of ["/codex/models", "/models"]) {
		const models = await discoverModels(config, credentials, path);
		if (models !== undefined) {
			lastCatalogSource = "remote";
			reasoningByModel = reasoningLookup(models);
			return copyModels(models);
		}
	}
	reasoningByModel = reasoningLookup(FALLBACK_MODELS);
	lastCatalogSource = "bundled";
	return copyModels(FALLBACK_MODELS);
}

/** Returns whether the most recently discovered model supports Responses reasoning controls. */
export function supportsReasoning(modelId: string): boolean {
	return reasoningByModel.get(modelId) ?? false;
}

async function discoverModels(
	config: ProviderConfig,
	credentials: Credentials,
	path: string,
): Promise<Model[] | undefined> {
	try {
		const response = await fetchWithTimeout(
			modelsUrl(config.codexBaseUrl, path, config.clientVersion),
			{
				method: "GET",
				headers: {
					...authHeaders(credentials),
					"openai-beta": "responses=experimental",
					originator: config.originator,
					version: config.clientVersion,
					accept: "application/json",
				},
			},
			config.requestTimeoutMs,
		);
		if (!response.ok) return undefined;
		return parseModels(await response.json());
	} catch {
		return undefined;
	}
}

function modelsUrl(baseUrl: string, path: string, clientVersion: string): URL {
	const url = new URL(baseUrl);
	url.pathname = `${url.pathname.replace(/\/+$/, "")}${path}`;
	url.search = new URLSearchParams({ client_version: clientVersion }).toString();
	return url;
}

function parseModels(value: unknown): Model[] | undefined {
	if (!isRecord(value)) return undefined;
	const entries = Array.isArray(value["models"])
		? value["models"]
		: Array.isArray(value["data"])
			? value["data"]
			: undefined;
	if (!entries) return undefined;
	const catalog = entries.flatMap(normalizeModel).sort(compareModels);
	return catalog.length > 0 ? catalog.map(({ priority: _priority, ...model }) => model) : undefined;
}

function normalizeModel(value: unknown): CatalogEntry[] {
	if (!isRecord(value)) return [];
	const id = nonEmptyString(value["slug"]) ?? nonEmptyString(value["id"]);
	if (!id || isHidden(value["visibility"])) return [];
	const thinking = thinkingMetadata(
		value["default_reasoning_level"],
		value["supported_reasoning_levels"],
	);
	return [
		{
			id,
			display_name: nonEmptyString(value["display_name"]) ?? id,
			context_window: positiveInteger(value["context_window"]) ?? contextWindowFallback(id),
			input_modalities: inputModalities(value["input_modalities"]),
			reasoning: thinking !== undefined,
			...(thinking === undefined ? {} : { thinking }),
			...(nonEmptyString(value["description"]) === undefined
				? {}
				: { description: nonEmptyString(value["description"]) }),
			...(nonEmptyString(value["pricing"]) === undefined
				? {}
				: { pricing: nonEmptyString(value["pricing"]) }),
			priority: finiteNumber(value["priority"]) ?? Number.MAX_SAFE_INTEGER,
		},
	];
}

function inputModalities(value: unknown): ("text" | "image")[] {
	if (value === undefined) return ["text", "image"];
	if (!Array.isArray(value) || value.some((entry) => entry !== "text" && entry !== "image")) {
		return ["text"];
	}
	const modalities = [...new Set(value)];
	return modalities.includes("text") ? modalities : ["text"];
}

function compareModels(left: CatalogEntry, right: CatalogEntry): number {
	return left.priority === right.priority
		? left.id.localeCompare(right.id)
		: left.priority - right.priority;
}

function contextWindowFallback(modelId: string): number {
	return modelId.toLowerCase().startsWith("gpt-5.6")
		? GPT_5_6_CONTEXT_WINDOW
		: DEFAULT_CONTEXT_WINDOW;
}

function thinkingMetadata(
	defaultLevel: unknown,
	supportedLevels: unknown,
): Model["thinking"] | undefined {
	const ids = Array.isArray(supportedLevels)
		? supportedLevels.flatMap((level) => {
				const id =
					typeof level === "string"
						? nonEmptyString(level)
						: isRecord(level)
							? nonEmptyString(level["id"])
							: undefined;
				return id && /^[a-z0-9][a-z0-9_-]{0,63}$/.test(id) ? [id] : [];
			})
		: [];
	const unique = [...new Set(ids)];
	const declaredDefault = nonEmptyString(defaultLevel);
	if (declaredDefault && !unique.includes(declaredDefault)) unique.push(declaredDefault);
	if (unique.length === 0) return undefined;
	const defaultId =
		declaredDefault && unique.includes(declaredDefault) ? declaredDefault : unique[0];
	if (defaultId === undefined) return undefined;
	return {
		default: defaultId,
		levels: unique.map((id) => ({ id, description: reasoningDescription(id) })),
	};
}

function reasoningDescription(id: string): string {
	return (
		{
			minimal: "Minimal reasoning",
			low: "Faster, lighter reasoning",
			medium: "Balanced reasoning",
			high: "Deeper reasoning",
			xhigh: "Maximum reasoning",
		}[id] ?? `Provider reasoning level ${id}`
	);
}

function isHidden(visibility: unknown): boolean {
	const value = nonEmptyString(visibility)?.toLowerCase();
	return value === "hide" || value === "hidden";
}

function nonEmptyString(value: unknown): string | undefined {
	if (typeof value !== "string") return undefined;
	const trimmed = value.trim();
	return trimmed || undefined;
}

function positiveInteger(value: unknown): number | undefined {
	return typeof value === "number" && Number.isSafeInteger(value) && value > 0 ? value : undefined;
}

function finiteNumber(value: unknown): number | undefined {
	return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}

function copyModels(models: readonly Model[]): Model[] {
	return models.map((model) => ({ ...model, input_modalities: [...model.input_modalities] }));
}

function reasoningLookup(models: readonly Model[]): Map<string, boolean> {
	return new Map(models.map((model) => [model.id, model.reasoning]));
}

function imageModel(model: Omit<Model, "input_modalities">): Model {
	return {
		...model,
		...(model.reasoning && model.thinking === undefined
			? { thinking: thinkingMetadata("medium", ["low", "medium", "high", "xhigh"]) }
			: {}),
		input_modalities: ["text", "image"],
	};
}
