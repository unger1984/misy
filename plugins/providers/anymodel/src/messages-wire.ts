/** Strict OpenAI Chat Completions message translation for core-owned history. */
import { type ImageAttachment, imageAttachments, imageDataUrl, isRecord, type Json } from "./types";

/** Removes Misy-only fields and expands tool history into OpenAI-compatible messages. */
export function openAiMessages(
	messages: readonly Record<string, unknown>[],
): readonly Record<string, unknown>[] {
	const mapped: Record<string, unknown>[] = [];
	for (const message of messages) {
		switch (message["role"]) {
			case "assistant":
				mapped.push(assistantMessage(message));
				break;
			case "tool":
				mapped.push(...toolResultMessages(message));
				break;
			case "system":
				mapped.push({ role: "system", content: text(message["content"]) });
				break;
			default:
				mapped.push(userMessage(message));
		}
	}
	return mapped;
}

function assistantMessage(message: Record<string, unknown>): Record<string, unknown> {
	const mapped: Record<string, unknown> = {
		role: "assistant",
		content: text(message["content"]),
	};
	const calls = Array.isArray(message["tool_calls"])
		? message["tool_calls"].flatMap(openAiToolCall)
		: [];
	if (calls.length > 0) mapped["tool_calls"] = calls;
	return mapped;
}

function openAiToolCall(value: unknown): Record<string, unknown>[] {
	if (!isRecord(value)) return [];
	const id = value["id"];
	const name = value["name"];
	if (typeof id !== "string" || typeof name !== "string") return [];
	return [
		{
			id,
			type: "function",
			function: { name, arguments: JSON.stringify(value["arguments"] ?? {}) },
		},
	];
}

function userMessage(message: Record<string, unknown>): Record<string, unknown> {
	const content = text(message["content"]);
	const attachments = imageAttachments(message["attachments"]);
	return {
		role: "user",
		content: attachments.length === 0 ? content : contentParts(content, attachments),
	};
}

function toolResultMessages(message: Record<string, unknown>): Record<string, unknown>[] {
	if (!Array.isArray(message["tool_results"])) return [];
	return message["tool_results"].flatMap((value) => {
		if (!isRecord(value) || typeof value["tool_call_id"] !== "string") return [];
		const result = {
			role: "tool",
			tool_call_id: value["tool_call_id"],
			content: text(value["content"]),
		};
		const attachments = imageAttachments(value["attachments"]);
		return attachments.length === 0
			? [result]
			: [result, toolImageMessage(value["tool_call_id"], attachments)];
	});
}

function toolImageMessage(
	id: string,
	attachments: readonly ImageAttachment[],
): Record<string, unknown> {
	return {
		role: "user",
		content: contentParts(`Images from tool result ${id}:`, attachments),
	};
}

function contentParts(text: string, attachments: readonly ImageAttachment[]): Json[] {
	const parts: Json[] = text.length > 0 ? [{ type: "text", text }] : [];
	for (const attachment of attachments) {
		parts.push({ type: "image_url", image_url: { url: imageDataUrl(attachment) } });
	}
	return parts;
}

function text(value: unknown): string {
	return typeof value === "string" ? value : "";
}
