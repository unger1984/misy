/** Deadline-aware HTTP helpers shared by provider plugins. */

/**
 * Fetches with both a deadline and an optional caller cancellation signal.
 *
 * Every provider HTTP request must carry a deadline: an unbounded wait on a remote endpoint is a
 * hang the core cannot recover from, because the core enforces its own JSON-RPC deadline.
 */
export async function fetchWithTimeout(
	input: URL | string,
	init: RequestInit,
	timeoutMs: number,
): Promise<Response> {
	const deadline = AbortSignal.timeout(timeoutMs);
	const signal =
		init.signal === undefined || init.signal === null
			? deadline
			: AbortSignal.any([init.signal, deadline]);
	return await fetch(input, { ...init, signal });
}

/** Joins a provider base URL with an API path, tolerating a missing trailing slash. */
export function endpointUrl(baseUrl: string, path: string): URL {
	return new URL(path, baseUrl.endsWith("/") ? baseUrl : `${baseUrl}/`);
}
