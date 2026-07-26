/** Deadline-aware HTTP transport used by every Kimi remote request. */

/** Fetches a Kimi endpoint with both a deadline and optional caller cancellation. */
export async function fetchWithTimeout(
	input: URL | string,
	init: RequestInit,
	timeoutMs: number,
): Promise<Response> {
	const deadline = AbortSignal.timeout(timeoutMs);
	const signal = init.signal ? AbortSignal.any([init.signal, deadline]) : deadline;
	return await fetch(input, { ...init, signal });
}
