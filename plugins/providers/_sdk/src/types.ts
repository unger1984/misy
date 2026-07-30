/**
 * Protocol-domain types shared by TypeScript provider plugins.
 *
 * These types model the Misy provider protocol boundary: values arrive from the core as
 * `unknown` JSON and are narrowed once, here, before any provider adapter sees them.
 */

/** JSON values retained verbatim in Misy's opaque credential store. */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** Base JSON object the core echoes back opaquely to the provider that issued it. */
export type ProviderCredentials = Record<string, Json>;

/** OAuth credentials the core echoes back opaquely to the provider that issued them. */
export type OAuthCredentials = ProviderCredentials & {
	access_token: string;
	refresh_token?: string;
	expires_at?: number;
	type: "oauth";
};

/** Backward-compatible name for bundled OAuth provider credentials. */
export type Credentials = OAuthCredentials;

/** API-key credentials owned by providers that authenticate with a static secret. */
export type ApiKeyCredentials = ProviderCredentials & {
	api_key: string;
	type: "api_key";
};

/** Narrows an opaque credential value for one provider adapter. */
export type CredentialParser<TCredentials extends ProviderCredentials> = (
	value: unknown,
) => TCredentials | undefined;

/** A core-owned tool definition forwarded to the remote model. */
export type ToolDefinition = {
	name: string;
	description: string;
	input_schema: Json;
};

/** A PNG image carried inline by image-input capability version 1. */
export type ImageAttachment = {
	type: "image";
	media_type: "image/png";
	data_base64: string;
};

const MAX_IMAGE_BYTES = 5 * 1024 * 1024;
const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a] as const;

/** A normalized tool result with optional image-input version 1 attachments. */
export type ToolResult = Record<string, Json> & {
	attachments?: ImageAttachment[];
};

/** A normalized chat message with optional message or nested tool-result images. */
export type ChatMessage = Record<string, Json> & {
	attachments?: ImageAttachment[];
	tool_results?: ToolResult[];
};

/** A validated `chat.start` request handed to the provider adapter. */
export type ChatRequest<TCredentials extends ProviderCredentials = OAuthCredentials> = {
	model_id: string;
	/** Optional provider-owned thinking level, negotiated through capability `thinking` v1. */
	thinking?: string;
	/** Optional core-owned output ceiling used by maintenance turns such as compaction. */
	max_output_tokens?: number;
	messages: readonly ChatMessage[];
	tools: readonly ToolDefinition[];
	credentials: TCredentials;
};

/** Delivers one stream notification correlated to its `chat.start` JSON-RPC request. */
export type Notify = (method: string, params: Record<string, Json>) => void;

/** Narrows untrusted JSON or process input to a key-value record. */
export function isRecord(value: unknown): value is Record<string, Json> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}

/** Narrows one untrusted attachment to the image-input version 1 PNG contract. */
export function isImageAttachment(value: unknown): value is ImageAttachment {
	if (!isRecord(value)) return false;
	return (
		value["type"] === "image" &&
		value["media_type"] === "image/png" &&
		typeof value["data_base64"] === "string" &&
		isBase64(value["data_base64"])
	);
}

/** Returns valid image attachments from an already validated message field. */
export function imageAttachments(value: unknown): ImageAttachment[] {
	return Array.isArray(value) ? value.filter(isImageAttachment) : [];
}

/** Encodes an inline image attachment as the data URL accepted by remote provider APIs. */
export function imageDataUrl(attachment: ImageAttachment): string {
	return `data:${attachment.media_type};base64,${attachment.data_base64}`;
}

function isBase64(value: string): boolean {
	if (value.length === 0 || value.length % 4 !== 0) return false;
	const padding = value.endsWith("==") ? 2 : value.endsWith("=") ? 1 : 0;
	if ((value.length / 4) * 3 - padding > MAX_IMAGE_BYTES) return false;
	for (let index = 0; index < value.length - padding; index += 1) {
		if (!isBase64Code(value.charCodeAt(index))) return false;
	}
	const bytes = Buffer.from(value, "base64");
	return PNG_SIGNATURE.every((byte, index) => bytes[index] === byte);
}

function isBase64Code(code: number): boolean {
	return (
		(code >= 0x41 && code <= 0x5a) ||
		(code >= 0x61 && code <= 0x7a) ||
		(code >= 0x30 && code <= 0x39) ||
		code === 0x2b ||
		code === 0x2f
	);
}
