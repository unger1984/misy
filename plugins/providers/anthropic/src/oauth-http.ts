/** Minimal OAuth token transport that preserves Anthropic's no-`Accept` request fingerprint. */
import { connect as connectTcp } from "node:net";
import { connect as connectTls } from "node:tls";

// A token endpoint answers with a small JSON document; anything past this cap is a
// misbehaving or hostile endpoint that would otherwise burn CPU and memory until the deadline.
const MAX_RESPONSE_BYTES = 256 * 1024;
const HEADER_BOUNDARY = Buffer.from("\r\n\r\n");
const CHUNK_TERMINATOR = Buffer.from("\r\n0\r\n");

/** A completed OAuth token HTTP response. */
export type TokenResponse = { status: number; body: string };

/**
 * Posts a JSON OAuth token request without an `Accept` header.
 *
 * Throws when the connection, response, or deadline fails, when a request field contains CR/LF,
 * or when the response grows past the 256 KiB cap. This deliberately uses raw HTTP because
 * Bun's Fetch and HTTP compatibility layers synthesize a default `Accept` header, which changes
 * this endpoint's observed Claude Code fingerprint.
 */
export async function postOAuthJson(
	url: URL,
	headers: Record<string, string>,
	body: string,
	timeoutMs: number,
): Promise<TokenResponse> {
	const deadline = new AbortController();
	const timer = setTimeout(() => deadline.abort(), timeoutMs);
	try {
		return await new Promise<TokenResponse>((resolve, reject) => {
			// Built before any socket work so an invalid field rejects the promise instead
			// of throwing inside a connect event handler.
			const request = requestText(url, headers, body);
			const socket = openSocket(url);
			const incoming = createResponseAccumulator();
			let settled = false;
			const finish = (response: TokenResponse) => {
				if (settled) return;
				settled = true;
				socket.destroy();
				resolve(response);
			};
			const fail = (cause: unknown) => {
				if (settled) return;
				settled = true;
				socket.destroy();
				reject(cause instanceof Error ? cause : new Error("Anthropic OAuth token request failed"));
			};
			const abort = () => socket.destroy(new Error("Anthropic OAuth token request timed out"));
			deadline.signal.addEventListener("abort", abort, { once: true });
			socket.once("error", fail);
			socket.on("data", (chunk: Uint8Array) => {
				try {
					const response = incoming.push(chunk);
					if (response !== undefined) finish(response);
				} catch (cause) {
					fail(cause);
				}
			});
			socket.once("end", () => {
				try {
					finish(parseResponse(incoming.text()));
				} catch (cause) {
					fail(cause);
				}
			});
			socket.once(url.protocol === "https:" ? "secureConnect" : "connect", () => {
				socket.write(request);
			});
		});
	} finally {
		clearTimeout(timer);
	}
}

function openSocket(url: URL) {
	const port = Number(url.port) || (url.protocol === "https:" ? 443 : 80);
	if (url.protocol === "https:")
		return connectTls({ host: url.hostname, port, servername: url.hostname });
	if (url.protocol === "http:") return connectTcp({ host: url.hostname, port });
	throw new Error(`Anthropic OAuth token endpoint uses unsupported protocol: ${url.protocol}`);
}

function requestText(url: URL, headers: Record<string, string>, body: string): string {
	const host = url.port.length > 0 ? `${url.hostname}:${url.port}` : url.hostname;
	const target = `${url.pathname}${url.search}`;
	const headerLines = Object.entries(headers).map(([name, value]) => `${name}: ${value}`);
	// Raw request assembly has no encoder to reject control bytes: a CR/LF in an interpolated
	// field would split the request. Callers pass constants today, so this guards future inputs.
	if ([target, host, ...headerLines].some((field) => /[\r\n]/.test(field))) {
		throw new Error("Refusing to send the Anthropic OAuth token request: CR/LF in an HTTP field");
	}
	const lines = [
		`POST ${target} HTTP/1.1`,
		`Host: ${host}`,
		"Connection: close",
		`Content-Length: ${Buffer.byteLength(body)}`,
		...headerLines,
		"",
		body,
	];
	return lines.join("\r\n");
}

function parseResponse(raw: string): TokenResponse {
	const boundary = raw.indexOf("\r\n\r\n");
	if (boundary < 0)
		throw new Error("Anthropic OAuth token endpoint returned an invalid HTTP response");
	const header = raw.slice(0, boundary);
	const status = Number(/^HTTP\/\d\.\d\s+(\d{3})/.exec(header)?.[1]);
	if (!Number.isInteger(status))
		throw new Error("Anthropic OAuth token endpoint returned an invalid status");
	const body = raw.slice(boundary + 4);
	return {
		status,
		body: /\r\ntransfer-encoding:\s*chunked(?:\r\n|$)/i.test(header) ? dechunk(body) : body,
	};
}

/**
 * Collects a raw response and completes it as soon as its framing allows.
 *
 * `push` throws when the response grows past {@link MAX_RESPONSE_BYTES}.
 */
function createResponseAccumulator() {
	const chunks: Uint8Array[] = [];
	let received = 0;
	let headerEnd = -1;
	let chunked = false;
	let contentLength = Number.NaN;
	let terminated = false;
	let tail = Buffer.alloc(0);

	const push = (chunk: Uint8Array): TokenResponse | undefined => {
		chunks.push(chunk);
		received += chunk.length;
		if (received > MAX_RESPONSE_BYTES) {
			throw new Error(
				`Anthropic OAuth token endpoint response exceeded the ${MAX_RESPONSE_BYTES}-byte limit`,
			);
		}
		// Scan only the new chunk plus a short overlap from the previous one; concatenating
		// everything received so far on every chunk would make a large response quadratic.
		const window = Buffer.concat([tail, chunk]);
		const base = received - window.length;
		tail = Buffer.from(window.subarray(Math.max(0, window.length - CHUNK_TERMINATOR.length)));
		if (headerEnd < 0) {
			const hit = window.indexOf(HEADER_BOUNDARY);
			if (hit < 0) return undefined;
			headerEnd = base + hit;
			// Headers are parsed once, the moment their boundary is seen.
			const header = Buffer.concat(chunks).subarray(0, headerEnd).toString();
			chunked = /\r\ntransfer-encoding:\s*chunked(?:\r\n|$)/i.test(header);
			contentLength = Number(/\r\ncontent-length:\s*(\d+)(?:\r\n|$)/i.exec(header)?.[1]);
		}
		if (chunked) {
			if (!terminated) {
				const bodyOffset = Math.max(headerEnd + 4, base) - base;
				terminated = window.indexOf(CHUNK_TERMINATOR, bodyOffset) >= 0;
			}
			if (terminated) return parseResponse(Buffer.concat(chunks).toString());
			return undefined;
		}
		if (Number.isSafeInteger(contentLength) && received - (headerEnd + 4) >= contentLength) {
			return parseResponse(Buffer.concat(chunks).toString());
		}
		return undefined;
	};

	const text = (): string => Buffer.concat(chunks).toString();

	return { push, text };
}

function dechunk(body: string): string {
	let cursor = 0;
	let decoded = "";
	while (cursor < body.length) {
		const lineEnd = body.indexOf("\r\n", cursor);
		if (lineEnd < 0) throw new Error("Anthropic OAuth token endpoint returned malformed chunks");
		const size = Number.parseInt(body.slice(cursor, lineEnd), 16);
		if (!Number.isSafeInteger(size) || size < 0) {
			throw new Error("Anthropic OAuth token endpoint returned an invalid chunk size");
		}
		if (size === 0) return decoded;
		const start = lineEnd + 2;
		const end = start + size;
		if (end > body.length) throw new Error("Anthropic OAuth token endpoint ended a chunk early");
		decoded += body.slice(start, end);
		cursor = end + 2;
	}
	throw new Error("Anthropic OAuth token endpoint did not terminate its chunked response");
}
