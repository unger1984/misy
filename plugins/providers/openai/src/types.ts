/**
 * Shared provider-domain types re-exported from the protocol SDK.
 *
 * ChatGPT-specific credential fields (`chatgpt_account_id`, `id_token`) ride along through the
 * SDK's open credential shape; the provider reads them at its own API boundary.
 */
export {
	type ChatMessage,
	type ChatRequest,
	type Credentials,
	type ImageAttachment,
	imageAttachments,
	imageDataUrl,
	isRecord,
	type Json,
	type Notify,
	type ToolDefinition,
	type ToolResult,
} from "@misy/provider-sdk";
