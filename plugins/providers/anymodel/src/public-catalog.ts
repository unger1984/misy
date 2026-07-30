/** Cached enrichment from AnyModel's official public model and pricing pages. */

import { fetchWithTimeout } from "@misy/provider-sdk";
import type { ProviderConfig } from "./config";

export type PublicModelMetadata = {
	displayName: string;
	description?: string;
	pricing: string;
	contextWindow?: number;
};

type CacheEntry = {
	expiresAt: number;
	models: ReadonlyMap<string, PublicModelMetadata>;
};

const cache = new Map<string, CacheEntry>();
const refreshes = new Map<string, Promise<ReadonlyMap<string, PublicModelMetadata>>>();
const CARD_PATTERN = new RegExp(
	[
		'"card":\\{"id":"((?:\\\\.|[^"\\\\])*)",',
		'"name":"((?:\\\\.|[^"\\\\])*)",',
		'"tagline":(?:"((?:\\\\.|[^"\\\\])*)"|null),',
		'[\\s\\S]*?"price":\\{([\\s\\S]*?)\\},',
		'"context":(?:"([^"]*)"|null)',
	].join(""),
	"g",
);
// biome-ignore lint/complexity/useRegexLiterals: construction keeps the protocol boundary readable.
const NEXT_FRAME_PATTERN = new RegExp(
	'<script[^>]*>self\\.__next_f\\.push\\((\\[1,"(?:\\\\.|[^"\\\\])*"\\])\\)</script>',
	"g",
);

/** Returns cached official metadata, retaining stale data across transient page failures. */
export async function publicModelMetadata(
	config: ProviderConfig,
): Promise<ReadonlyMap<string, PublicModelMetadata>> {
	const url = config.publicCatalogUrl;
	if (!url) return new Map();
	const cached = cache.get(url);
	if (cached && cached.expiresAt > Date.now()) return cached.models;
	const refreshing = refreshes.get(url);
	if (refreshing) return await refreshing;
	const refresh = fetchCatalog(url, config)
		.then((models) => {
			cache.set(url, { expiresAt: Date.now() + config.publicCatalogTtlMs, models });
			return models;
		})
		.catch(() => cached?.models ?? new Map<string, PublicModelMetadata>())
		.finally(() => refreshes.delete(url));
	refreshes.set(url, refresh);
	return await refresh;
}

async function fetchCatalog(
	baseUrl: string,
	config: ProviderConfig,
): Promise<ReadonlyMap<string, PublicModelMetadata>> {
	const first = await fetchPage(baseUrl, config.publicCatalogTimeoutMs);
	const pageCount = maximumPage(first);
	const remaining = await Promise.all(
		Array.from({ length: pageCount - 1 }, (_, index) =>
			fetchPage(pageUrl(baseUrl, index + 2), config.publicCatalogTimeoutMs),
		),
	);
	const models = new Map<string, PublicModelMetadata>();
	for (const html of [first, ...remaining]) {
		for (const [id, metadata] of parsePublicCatalog(html)) models.set(id, metadata);
	}
	if (models.size === 0) throw new Error("AnyModel public catalog did not contain model cards");
	return models;
}

async function fetchPage(url: string, timeoutMs: number): Promise<string> {
	const response = await fetchWithTimeout(
		url,
		{ headers: { accept: "text/html", "accept-language": "en" } },
		timeoutMs,
	);
	if (!response.ok) throw new Error(`AnyModel public catalog request failed (${response.status})`);
	return await response.text();
}

function maximumPage(html: string): number {
	let maximum = 1;
	for (const match of html.matchAll(/[?&]page=(\d+)/g)) {
		const page = Number(match[1]);
		if (Number.isSafeInteger(page)) maximum = Math.max(maximum, Math.min(page, 10));
	}
	return maximum;
}

function pageUrl(baseUrl: string, page: number): string {
	const url = new URL(baseUrl);
	url.searchParams.set("page", String(page));
	return url.toString();
}

function parsePublicCatalog(html: string): Map<string, PublicModelMetadata> {
	const payload = nextPayload(html);
	const cards = new Map<string, PublicModelMetadata>();
	for (const match of payload.matchAll(CARD_PATTERN)) {
		const [id, name, tagline, priceBody, context] = match.slice(1);
		if (!id || !name || priceBody === undefined) continue;
		const label = /"label":"((?:\\.|[^"\\])*)"/.exec(priceBody)?.[1];
		if (!label) continue;
		cards.set(decodeJson(id), {
			displayName: decodeJson(name),
			...(tagline ? { description: decodeJson(tagline) } : {}),
			pricing: decodeJson(label).replaceAll("$$", "$"),
			...(context ? { contextWindow: parseContext(context) } : {}),
		});
	}
	return cards;
}

function nextPayload(html: string): string {
	const chunks = [];
	for (const match of html.matchAll(NEXT_FRAME_PATTERN)) {
		try {
			const frame = JSON.parse(match[1] ?? "") as unknown;
			if (Array.isArray(frame) && typeof frame[1] === "string") chunks.push(frame[1]);
		} catch {
			// A malformed stream chunk is ignored; complete cards from other chunks remain usable.
		}
	}
	return chunks.join("");
}

function decodeJson(value: string): string {
	try {
		return JSON.parse(`"${value}"`) as string;
	} catch {
		return value;
	}
}

function parseContext(value: string): number | undefined {
	const match = /^(\d+(?:\.\d+)?)([KM])$/i.exec(value.trim());
	if (!match) return undefined;
	const amount = Number(match[1]);
	const multiplier = match[2]?.toUpperCase() === "M" ? 1_000_000 : 1_000;
	const tokens = amount * multiplier;
	return Number.isSafeInteger(tokens) && tokens > 0 ? tokens : undefined;
}
