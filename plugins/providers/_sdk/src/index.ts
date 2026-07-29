/**
 * Misy provider SDK: shared protocol transport for TypeScript provider plugins.
 *
 * This package is an optional implementation library for TypeScript plugins only. It is not part
 * of the provider protocol: plugins in other languages implement the same JSON-RPC contract
 * independently, and every plugin remains a standalone process with its own manifest.
 */
export { oauthCredentials, requireOauthCredentials } from "./credentials";
export { endpointUrl, fetchWithTimeout } from "./http";
export { preferredDefaultModel } from "./models";
export { type ProviderAdapter, serve } from "./serve";
export {
	type ChatMessage,
	type ChatRequest,
	type Credentials,
	type ImageAttachment,
	imageAttachments,
	imageDataUrl,
	isImageAttachment,
	isRecord,
	type Json,
	type Notify,
	type ToolDefinition,
	type ToolResult,
} from "./types";
