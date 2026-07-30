/** OpenAI Responses request construction and SSE normalization. */
import { supportsReasoning } from "./model-catalog";
import {
	type ImageAttachment,
	imageAttachments,
	imageDataUrl,
	isRecord,
	type Json,
	type Notify,
	type ToolDefinition,
} from "./types";

/** Builds the complete Responses body required by the subscription backend. */
export function createResponsesRequest(
	model: string,
	messages: readonly Record<string, unknown>[],
	tools: readonly ToolDefinition[],
	thinking?: string,
	maxOutputTokens?: number,
): Record<string, Json> {
	const instructions = messages
		.filter((message) => message["role"] === "system")
		.map((message) => (typeof message["content"] === "string" ? message["content"] : ""))
		.join("\n\n");
	const input = responseInput(messages);
	return {
		model,
		instructions,
		stream: true,
		store: false,
		tool_choice: "auto",
		parallel_tool_calls: true,
		include: ["reasoning.encrypted_content"],
		...reasoningOptions(model, thinking),
		...(maxOutputTokens === undefined ? {} : { max_output_tokens: maxOutputTokens }),
		text: { verbosity: "medium" },
		input,
		tools: tools.map((tool) => ({
			type: "function",
			name: tool.name,
			description: tool.description,
			parameters: tool.input_schema,
		})),
	};
}

function reasoningOptions(model: string, thinking?: string): Record<string, Json> {
	if (!supportsReasoning(model)) return {};
	return {
		reasoning: { effort: thinking ?? "medium", summary: "auto" },
		stream_options: { reasoning_summary_delivery: "sequential_cutoff" },
	};
}

/** Converts Responses SSE events to Misy notifications. */
export async function notifyResponseEvents(
	stream: ReadableStream<Uint8Array>,
	requestId: number,
	notify: Notify,
): Promise<Json> {
	const reader = stream.getReader();
	const decoder = new TextDecoder();
	let pending = "";
	let metadata: Json = { completed: false };
	try {
		while (true) {
			const read = await reader.read();
			pending += decoder.decode(read.value, { stream: !read.done });
			const parts = pending.split(/\r?\n\r?\n/);
			pending = parts.pop() ?? "";
			for (const part of parts) {
				const event = part.match(/^event:\s*(.+)$/m)?.[1] ?? "message";
				const raw = part.match(/^data:\s*(.+)$/m)?.[1] ?? "{}";
				const value: unknown = JSON.parse(raw);
				const data = record(value);
				if (event === "response.output_text.delta" && typeof data["delta"] === "string")
					notify("text_delta", { request_id: requestId, delta: data["delta"] });
				if (event === "response.output_item.done") notifyToolCall(data, requestId, notify);
				if (event === "response.completed") {
					metadata = data;
					const response = record(data["response"]);
					const usage = record(response["usage"]);
					notify("completed", {
						request_id: requestId,
						metadata,
						...(typeof usage["input_tokens"] === "number"
							? { input_tokens: usage["input_tokens"] }
							: {}),
						...(typeof usage["output_tokens"] === "number"
							? { output_tokens: usage["output_tokens"] }
							: {}),
					});
				}
				if (event === "response.failed" || event === "error")
					throw new Error(responseErrorMessage(data));
			}
			if (read.done) break;
		}
		// SSE events end with a blank line, so a non-empty remainder at EOF means the
		// connection dropped mid-event; completing silently would hide the truncation.
		if (pending.trim().length > 0) {
			throw new Error("OpenAI Responses stream ended mid-event");
		}
	} finally {
		reader.releaseLock();
	}
	return metadata;
}

function record(value: unknown): Record<string, Json> {
	return isRecord(value) ? value : {};
}
function responseInput(messages: readonly Record<string, unknown>[]): Json[] {
	const input: Json[] = [];
	for (const message of messages) {
		if (message["role"] === "system") continue;
		input.push(...reasoningInputs(message["provider_metadata"]));
		if (message["role"] === "assistant") {
			const content = typeof message["content"] === "string" ? message["content"] : "";
			if (content.length > 0) input.push({ role: "assistant", content });
			for (const call of Array.isArray(message["tool_calls"]) ? message["tool_calls"] : []) {
				const value = record(call);
				input.push({
					type: "function_call",
					call_id: typeof value["id"] === "string" ? value["id"] : "",
					name: typeof value["name"] === "string" ? value["name"] : "",
					arguments: JSON.stringify(value["arguments"] ?? {}),
				});
			}
			continue;
		}
		if (message["role"] === "tool" && Array.isArray(message["tool_results"])) {
			for (const result of message["tool_results"]) {
				const value = record(result);
				input.push({
					type: "function_call_output",
					call_id: typeof value["tool_call_id"] === "string" ? value["tool_call_id"] : "",
					output: functionOutput(value),
				});
			}
			continue;
		}
		input.push({
			role: typeof message["role"] === "string" ? message["role"] : "user",
			content: messageContent(message),
		});
	}
	return input;
}

function messageContent(message: Record<string, unknown>): Json {
	const text = typeof message["content"] === "string" ? message["content"] : "";
	const attachments = imageAttachments(message["attachments"]);
	if (message["role"] !== "user" || attachments.length === 0) return text;
	return richContent(text, attachments);
}

function functionOutput(result: Record<string, Json>): Json {
	const text = typeof result["content"] === "string" ? result["content"] : "";
	const attachments = imageAttachments(result["attachments"]);
	return attachments.length === 0 ? text : richContent(text, attachments);
}

function richContent(text: string, attachments: readonly ImageAttachment[]): Json[] {
	const content: Json[] = text.length > 0 ? [{ type: "input_text", text }] : [];
	for (const attachment of attachments) {
		content.push({ type: "input_image", image_url: imageDataUrl(attachment), detail: "high" });
	}
	return content;
}

function responseErrorMessage(data: Record<string, Json>): string {
	const response = record(data["response"]);
	const responseError = record(response["error"]);
	const error = record(data["error"]);
	for (const candidate of [responseError["message"], error["message"], data["message"]]) {
		if (typeof candidate === "string" && candidate.trim().length > 0) return candidate;
	}
	return "OpenAI Responses stream failed";
}

function reasoningInputs(metadata: unknown): Json[] {
	const inputs: Json[] = [];
	const seen = new Set<string>();
	collectReasoningInputs(metadata, inputs, seen);
	return inputs;
}

function collectReasoningInputs(value: unknown, inputs: Json[], seen: Set<string>): void {
	if (Array.isArray(value)) {
		for (const item of value) collectReasoningInputs(item, inputs, seen);
		return;
	}
	if (value === null || typeof value !== "object") return;
	const item = record(value);
	const encrypted = item["encrypted_content"];
	if (item["type"] === "reasoning" && typeof encrypted === "string" && !seen.has(encrypted)) {
		seen.add(encrypted);
		// Responses validates opaque reasoning items as a unit. Replaying only the encrypted blob
		// drops server-owned fields such as `id` and `summary` and makes the next tool turn invalid.
		inputs.push({ ...item });
	}
	for (const nested of Object.values(item)) collectReasoningInputs(nested, inputs, seen);
}
function notifyToolCall(data: Record<string, Json>, requestId: number, notify: Notify): void {
	const item = record(data["item"]);
	if (item["type"] !== "function_call" || typeof item["name"] !== "string") return;
	const id = typeof item["call_id"] === "string" ? item["call_id"] : item["id"];
	if (typeof id !== "string") return;
	const raw = item["arguments"];
	let argumentsValue: Json = raw ?? {};
	if (typeof raw === "string") {
		try {
			// JSON.parse can only produce Json values, so assigning to the Json-typed binding
			// states a fact; the annotation sheds parse's `any` without an `as` cast.
			argumentsValue = JSON.parse(raw);
		} catch {
			argumentsValue = { raw };
		}
	}
	notify("tool_call", { request_id: requestId, id, name: item["name"], arguments: argumentsValue });
}
