/** Minimal OAuth token transport that preserves Anthropic's no-`Accept` request fingerprint. */
import { connect as connectTcp } from "node:net";
import { connect as connectTls } from "node:tls";

/** A completed OAuth token HTTP response. */
export type TokenResponse = { status: number; body: string };

/**
 * Posts a JSON OAuth token request without an `Accept` header.
 *
 * Throws when the connection, response, or deadline fails. This deliberately uses raw HTTP because
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
			const socket = openSocket(url);
			const chunks: Uint8Array[] = [];
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
				chunks.push(chunk);
				try {
					const response = completeResponse(Buffer.concat(chunks).toString());
					if (response !== undefined) finish(response);
				} catch (cause) {
					fail(cause);
				}
			});
			socket.once("end", () => {
				try {
					finish(parseResponse(Buffer.concat(chunks).toString()));
				} catch (cause) {
					fail(cause);
				}
			});
			socket.once(url.protocol === "https:" ? "secureConnect" : "connect", () => {
				socket.write(requestText(url, headers, body));
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
	const lines = [
		`POST ${url.pathname}${url.search} HTTP/1.1`,
		`Host: ${host}`,
		"Connection: close",
		`Content-Length: ${Buffer.byteLength(body)}`,
		...Object.entries(headers).map(([name, value]) => `${name}: ${value}`),
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

function completeResponse(raw: string): TokenResponse | undefined {
	const boundary = raw.indexOf("\r\n\r\n");
	if (boundary < 0) return undefined;
	const header = raw.slice(0, boundary);
	const body = raw.slice(boundary + 4);
	if (/\r\ntransfer-encoding:\s*chunked(?:\r\n|$)/i.test(header)) {
		return body.includes("\r\n0\r\n") ? parseResponse(raw) : undefined;
	}
	const length = Number(/\r\ncontent-length:\s*(\d+)(?:\r\n|$)/i.exec(header)?.[1]);
	return Number.isSafeInteger(length) && Buffer.byteLength(body) >= length
		? parseResponse(raw)
		: undefined;
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
