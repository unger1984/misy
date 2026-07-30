/** Authenticated AnyModel catalog retrieval and conservative response normalization. */
import { endpointUrl, fetchWithTimeout } from "@misy/provider-sdk";
import type { ProviderConfig } from "./config";
import { type ApiKeyCredentials, isRecord, type Model } from "./types";

const DEFAULT_CONTEXT_WINDOW = 128_000;

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
	return models;
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
		},
	];
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
	return DEFAULT_CONTEXT_WINDOW;
}

function inputModalities(entry: Record<string, unknown>): ("text" | "image")[] {
	const value = entry["input_modalities"];
	if (!Array.isArray(value) || new Set(value).size !== value.length) return ["text"];
	if (!value.every((item) => item === "text" || item === "image")) return ["text"];
	return value.includes("text") && value.includes("image") ? ["text", "image"] : ["text"];
}
