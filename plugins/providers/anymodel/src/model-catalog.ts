/** Authenticated AnyModel catalog retrieval and conservative response normalization. */
import { endpointUrl, fetchWithTimeout } from "@misy/provider-sdk";
import type { ProviderConfig } from "./config";
import { type PublicModelMetadata, publicModelMetadata } from "./public-catalog";
import { type ApiKeyCredentials, isRecord, type Model } from "./types";

/** Fetches and normalizes the non-empty authenticated AnyModel model catalog. */
export async function listModelCatalog(
	config: ProviderConfig,
	credentials: ApiKeyCredentials,
	signal?: AbortSignal,
): Promise<Model[]> {
	let response: Response;
	try {
		response = await fetchWithTimeout(
			endpointUrl(config.baseUrl, "models"),
			{
				method: "GET",
				signal,
				headers: { accept: "application/json", authorization: `Bearer ${credentials.api_key}` },
			},
			config.requestTimeoutMs,
		);
	} catch (cause) {
		if (signal?.aborted) throw cause;
		throw new Error("AnyModel model catalog request failed");
	}
	if (!response.ok) throw new Error(`AnyModel model catalog request failed (${response.status})`);
	let payload: unknown;
	try {
		payload = await response.json();
	} catch {
		throw new Error("AnyModel model catalog returned invalid JSON");
	}
	const models = parseModelCatalog(payload);
	if (models.length === 0) throw new Error("AnyModel model catalog contained no valid models");
	const metadata = await publicModelMetadata(config);
	return models.map((model) => enrichModel(model, metadata.get(model.id)));
}

/** Parses an OpenAI-style catalog without trusting optional vendor metadata. */
export function parseModelCatalog(payload: unknown): Model[] {
	if (!isRecord(payload) || !Array.isArray(payload["data"])) return [];
	return payload["data"].flatMap((entry) => normalizeModel(entry));
}

function normalizeModel(entry: unknown): Model[] {
	if (!isRecord(entry) || typeof entry["id"] !== "string" || entry["id"].trim().length === 0)
		return [];
	const id = entry["id"];
	const thinking = thinkingMetadata(entry, id);
	return [
		{
			id,
			display_name: displayName(entry, id),
			context_window: contextWindow(entry),
			input_modalities: inputModalities(entry),
			...(textMetadata(entry, "description") === undefined
				? {}
				: { description: textMetadata(entry, "description") }),
			...(textMetadata(entry, "pricing") === undefined
				? {}
				: { pricing: textMetadata(entry, "pricing") }),
			...(thinking === undefined ? {} : { thinking }),
		},
	];
}

function enrichModel(model: Model, metadata: PublicModelMetadata | undefined): Model {
	if (!metadata) return model;
	return {
		...model,
		display_name: model.display_name === model.id ? metadata.displayName : model.display_name,
		context_window: model.context_window || metadata.contextWindow || 0,
		...(model.description || !metadata.description ? {} : { description: metadata.description }),
		...(model.pricing ? {} : { pricing: metadata.pricing }),
	};
}

function thinkingMetadata(entry: Record<string, unknown>, id: string): Model["thinking"] {
	const requestedDefault = textMetadata(entry, "default_reasoning_level");
	const advertised = stringArray(entry["supported_reasoning_levels"]);
	const fallback = fallbackThinking(id);
	const levels = [
		...new Set([
			...(requestedDefault ? [requestedDefault] : []),
			...advertised,
			...fallback.levels,
		]),
	];
	if (levels.length === 0) return undefined;
	const preferredDefault = requestedDefault ?? fallback.default;
	const defaultLevel =
		preferredDefault && levels.includes(preferredDefault) ? preferredDefault : levels[0];
	return {
		default: defaultLevel ?? "low",
		levels: levels.map((level) => ({ id: level, description: thinkingDescription(level) })),
	};
}

function fallbackThinking(id: string): { default?: string; levels: string[] } {
	const normalized = id.toLowerCase();
	if (/(?:image|imagen|flux|black-forest)/.test(normalized)) return { levels: [] };
	if (/(?:^|\/)k3(?:-|$)/.test(normalized)) {
		return { default: "high", levels: ["low", "high", "max"] };
	}
	if (/gpt-5[.-]6/.test(normalized)) {
		return { levels: ["low", "medium", "high", "xhigh", "max"] };
	}
	if (/gpt-5/.test(normalized)) return { levels: ["low", "medium", "high", "xhigh"] };
	const reasoningFamily = /(?:claude|gemini|glm-5|qwen3)/.test(normalized);
	return { levels: reasoningFamily ? ["low", "high"] : [] };
}

function thinkingDescription(level: string): string {
	const descriptions: Record<string, string> = {
		low: "Faster, lighter reasoning",
		medium: "Balanced reasoning",
		high: "Deeper reasoning",
		xhigh: "Very deep reasoning",
		max: "Maximum reasoning effort",
	};
	return descriptions[level] ?? `Reasoning effort: ${level}`;
}

function stringArray(value: unknown): string[] {
	if (!Array.isArray(value)) return [];
	return value.filter(
		(entry): entry is string => typeof entry === "string" && entry.trim().length > 0,
	);
}

function textMetadata(entry: Record<string, unknown>, field: string): string | undefined {
	const value = entry[field];
	return typeof value === "string" && value.trim().length > 0 ? value : undefined;
}

function displayName(entry: Record<string, unknown>, id: string): string {
	for (const field of ["display_name", "name"]) {
		const value = entry[field];
		if (typeof value === "string" && value.trim().length > 0) return value;
	}
	return id;
}

function contextWindow(entry: Record<string, unknown>): number {
	for (const field of ["context_window", "context_length"]) {
		const value = entry[field];
		if (typeof value === "number" && Number.isInteger(value) && value > 0 && value <= 0xffffffff)
			return value;
	}
	return 0;
}

function inputModalities(entry: Record<string, unknown>): ("text" | "image")[] {
	const value = entry["input_modalities"];
	if (!Array.isArray(value) || new Set(value).size !== value.length) return ["text"];
	if (!value.every((item) => item === "text" || item === "image")) return ["text"];
	return value.includes("text") && value.includes("image") ? ["text", "image"] : ["text"];
}
