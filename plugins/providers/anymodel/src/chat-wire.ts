/** OpenAI-compatible AnyModel request construction and bounded SSE event normalization. */
import {
	type ImageAttachment,
	imageAttachments,
	imageDataUrl,
	isRecord,
	type Json,
	type Notify,
	type ToolDefinition,
} from "./types";

type ToolAccumulator = { id: string; name: string; arguments: string };

const MAX_SSE_FRAME_BYTES = 1024 * 1024;
const MAX_TOOL_CALLS = 128;
const MAX_TOOL_ARGUMENT_BYTES = 1024 * 1024;
const MAX_TOTAL_TOOL_ARGUMENT_BYTES = 4 * 1024 * 1024;
const MAX_TOOL_IDENTIFIER_BYTES = 4 * 1024;

/** Builds the upstream request while preserving the provider-local model identifier exactly. */
export function createChatRequest(
	model: string,
	messages: readonly Record<string, unknown>[],
	tools: readonly ToolDefinition[],
): Record<string, unknown> {
	return {
		model,
		messages: mapMessages(messages),
		tools: tools.map((tool) => ({
			type: "function",
			function: { name: tool.name, description: tool.description, parameters: tool.input_schema },
		})),
		stream: true,
	};
}

function mapMessages(
	messages: readonly Record<string, unknown>[],
): readonly Record<string, unknown>[] {
	if (!messages.some(hasAttachments)) return messages;
	const mapped: Record<string, unknown>[] = [];
	for (const message of messages) {
		const attachments = imageAttachments(message["attachments"]);
		if (message["role"] === "user" && attachments.length > 0) {
			mapped.push(withImageContent(message, attachments));
			continue;
		}
		mapped.push(stripAttachments(message));
		for (const result of toolImageResults(message))
			mapped.push(toolImageMessage(result.id, result.attachments));
	}
	return mapped;
}

function hasAttachments(message: Record<string, unknown>): boolean {
	return Object.hasOwn(message, "attachments") || toolImageResults(message).length > 0;
}

function withImageContent(
	message: Record<string, unknown>,
	attachments: readonly ImageAttachment[],
): Record<string, unknown> {
	const mapped = { ...message };
	delete mapped["attachments"];
	const text = typeof message["content"] === "string" ? message["content"] : "";
	mapped["content"] = contentParts(text, attachments);
	return mapped;
}

function stripAttachments(message: Record<string, unknown>): Record<string, unknown> {
	const mapped = { ...message };
	delete mapped["attachments"];
	if (!Array.isArray(message["tool_results"])) return mapped;
	mapped["tool_results"] = message["tool_results"].map((value) => {
		if (!isRecord(value)) return value;
		const result = { ...value };
		delete result["attachments"];
		return result;
	});
	return mapped;
}

function toolImageResults(
	message: Record<string, unknown>,
): { id: string; attachments: ImageAttachment[] }[] {
	if (message["role"] !== "tool" || !Array.isArray(message["tool_results"])) return [];
	return message["tool_results"].flatMap((value) => {
		if (!isRecord(value)) return [];
		const attachments = imageAttachments(value["attachments"]);
		if (attachments.length === 0) return [];
		return [
			{ id: typeof value["tool_call_id"] === "string" ? value["tool_call_id"] : "", attachments },
		];
	});
}

function toolImageMessage(
	id: string,
	attachments: readonly ImageAttachment[],
): Record<string, unknown> {
	const label = id ? `Images from tool result ${id}:` : "Images from a tool result:";
	return { role: "user", content: contentParts(label, attachments) };
}

function contentParts(text: string, attachments: readonly ImageAttachment[]): Json[] {
	const parts: Json[] = text.length > 0 ? [{ type: "text", text }] : [];
	for (const attachment of attachments) {
		parts.push({ type: "image_url", image_url: { url: imageDataUrl(attachment) } });
	}
	return parts;
}

/** Emits correlated text/tool notifications from a streaming OpenAI-compatible response. */
export async function notifyChatEvents(
	stream: ReadableStream<Uint8Array>,
	requestId: number,
	notify: Notify,
	apiKey = "",
): Promise<Json> {
	const reader = stream.getReader();
	const decoder = new TextDecoder();
	const tools = new Map<number, ToolAccumulator>();
	let pending = "";
	let done = false;
	try {
		readLoop: while (true) {
			const read = await reader.read();
			pending += decoder.decode(read.value, { stream: !read.done });
			const frames = pending.split(/\r?\n\r?\n/);
			pending = frames.pop() ?? "";
			assertFrameSize(pending);
			for (const frame of frames) {
				assertFrameSize(frame);
				if (processFrame(frame, tools, requestId, notify, apiKey)) {
					done = true;
					break readLoop;
				}
			}
			if (read.done) break;
		}
		if (!done && pending.trim()) {
			assertFrameSize(pending);
			done = processFrame(pending, tools, requestId, notify, apiKey);
		}
		if (done) await reader.cancel().catch(() => undefined);
		emitTools(tools, requestId, notify);
		const metadata: Json = { completed: true, finished_by: done ? "done" : "eof" };
		notify("completed", { request_id: requestId, metadata });
		return metadata;
	} finally {
		reader.releaseLock();
	}
}

function assertFrameSize(frame: string): void {
	if (Buffer.byteLength(frame, "utf8") > MAX_SSE_FRAME_BYTES) {
		throw new Error("AnyModel chat stream frame exceeded its size limit");
	}
}

function processFrame(
	frame: string,
	tools: Map<number, ToolAccumulator>,
	requestId: number,
	notify: Notify,
	apiKey: string,
): boolean {
	const payload = frame
		.split(/\r?\n/)
		.filter((line) => line.startsWith("data:"))
		.map((line) => line.slice(5).trim())
		.join("\n");
	if (!payload) return false;
	if (payload === "[DONE]") return true;
	let value: unknown;
	try {
		value = JSON.parse(payload);
	} catch {
		throw new Error("AnyModel chat stream contained invalid JSON");
	}
	if (!isRecord(value)) throw new Error("AnyModel chat stream event was not an object");
	if (isRecord(value["error"])) throw new Error(remoteMessage(value["error"], apiKey));
	if (!Array.isArray(value["choices"])) return false;
	for (const choice of value["choices"]) consumeChoice(choice, tools, requestId, notify);
	return value["choices"].some(
		(choice) =>
			isRecord(choice) &&
			typeof choice["finish_reason"] === "string" &&
			choice["finish_reason"].length > 0,
	);
}

function consumeChoice(
	value: unknown,
	tools: Map<number, ToolAccumulator>,
	requestId: number,
	notify: Notify,
): void {
	if (!isRecord(value) || !isRecord(value["delta"])) return;
	const delta = value["delta"];
	if (typeof delta["content"] === "string" && delta["content"].length > 0)
		notify("text_delta", { request_id: requestId, delta: delta["content"] });
	if (Array.isArray(delta["tool_calls"])) {
		for (const fragment of delta["tool_calls"]) appendTool(fragment, tools);
	}
}

function appendTool(value: unknown, tools: Map<number, ToolAccumulator>): void {
	if (!isRecord(value) || !validToolIndex(value["index"])) return;
	const index = value["index"];
	if (!tools.has(index) && tools.size >= MAX_TOOL_CALLS) {
		throw new Error("AnyModel chat stream returned too many tool calls");
	}
	const tool = tools.get(index) ?? { id: "", name: "", arguments: "" };
	if (typeof value["id"] === "string") {
		assertToolIdentifier(value["id"]);
		tool.id = value["id"];
	}
	if (isRecord(value["function"])) {
		const functionValue = value["function"];
		if (typeof functionValue["name"] === "string") {
			assertToolIdentifier(functionValue["name"]);
			tool.name = functionValue["name"];
		}
		if (typeof functionValue["arguments"] === "string") {
			appendToolArguments(tool, functionValue["arguments"], tools);
		}
	}
	tools.set(index, tool);
}

function validToolIndex(value: unknown): value is number {
	return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function assertToolIdentifier(value: string): void {
	if (Buffer.byteLength(value, "utf8") > MAX_TOOL_IDENTIFIER_BYTES) {
		throw new Error("AnyModel chat stream tool identifier exceeded its size limit");
	}
}

function appendToolArguments(
	tool: ToolAccumulator,
	fragment: string,
	tools: Map<number, ToolAccumulator>,
): void {
	const fragmentBytes = Buffer.byteLength(fragment, "utf8");
	if (Buffer.byteLength(tool.arguments, "utf8") + fragmentBytes > MAX_TOOL_ARGUMENT_BYTES) {
		throw new Error("AnyModel chat stream tool arguments exceeded their size limit");
	}
	const aggregateBytes = [...tools.values()].reduce(
		(total, entry) => total + Buffer.byteLength(entry.arguments, "utf8"),
		fragmentBytes,
	);
	if (aggregateBytes > MAX_TOTAL_TOOL_ARGUMENT_BYTES) {
		throw new Error("AnyModel chat stream tool arguments exceeded their aggregate limit");
	}
	tool.arguments += fragment;
}

function emitTools(tools: Map<number, ToolAccumulator>, requestId: number, notify: Notify): void {
	for (const [, tool] of [...tools.entries()].sort(([left], [right]) => left - right)) {
		if (!tool.id || !tool.name)
			throw new Error("AnyModel chat stream returned an incomplete tool call");
		notify("tool_call", {
			request_id: requestId,
			id: tool.id,
			name: tool.name,
			arguments: parseArguments(tool.arguments),
		});
	}
}

function parseArguments(raw: string): Json {
	if (!raw.trim()) return {};
	try {
		return JSON.parse(raw) as Json;
	} catch {
		return { raw };
	}
}

function remoteMessage(error: Record<string, unknown>, apiKey: string): string {
	const value = error["message"];
	return typeof value === "string"
		? `AnyModel chat stream failed: ${safeMessage(value, apiKey)}`
		: "AnyModel chat stream failed";
}

/** Prevents upstream streams from reflecting credentials or unbounded diagnostics into errors. */
export function safeMessage(message: string, apiKey = ""): string {
	const bounded = message.slice(0, 500);
	return (apiKey.length > 0 ? bounded.replaceAll(apiKey, "[redacted]") : bounded)
		.replace(/Bearer\s+[^\s,;]+/gi, "Bearer [redacted]")
		.replace(/(authorization|x-api-key)\s*[:=]\s*[^\s,;]+/gi, "$1: [redacted]");
}
