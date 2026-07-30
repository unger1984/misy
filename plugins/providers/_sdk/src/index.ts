/**
 * Misy provider SDK: shared protocol transport for TypeScript provider plugins.
 *
 * This package is an optional implementation library for TypeScript plugins only. It is not part
 * of the provider protocol: plugins in other languages implement the same JSON-RPC contract
 * independently, and every plugin remains a standalone process with its own manifest.
 */
export {
	apiKeyCredentials,
	oauthCredentials,
	requireCredentials,
	requireOauthCredentials,
} from "./credentials";
export { endpointUrl, fetchWithTimeout } from "./http";
export { referencePricing } from "./model-metadata";
export { preferredDefaultModel } from "./models";
export { type ProviderAdapter, serve } from "./serve";
export {
	type ApiKeyCredentials,
	type ChatMessage,
	type ChatRequest,
	type CredentialParser,
	type Credentials,
	type ImageAttachment,
	imageAttachments,
	imageDataUrl,
	isImageAttachment,
	isRecord,
	type Json,
	type Notify,
	type OAuthCredentials,
	type ProviderCredentials,
	type ToolDefinition,
	type ToolResult,
} from "./types";
