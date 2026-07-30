import { afterEach, expect, test } from "bun:test";
import type { ProviderConfig } from "../src/config";
import { publicModelMetadata } from "../src/public-catalog";

const servers: Array<ReturnType<typeof Bun.serve>> = [];

afterEach(() => {
	while (servers.length > 0) servers.pop()?.stop(true);
});

test("parses official prices, descriptions, and context windows", async () => {
	const { config } = catalogServer(() => catalogPage([modelCard("cx/gpt-5.6-sol", "1M")]));
	const models = await publicModelMetadata(config);

	expect(models.get("cx/gpt-5.6-sol")).toEqual({
		displayName: "GPT-5.6 Sol",
		description: "Powerful agentic coding model",
		pricing: "$0.10",
		contextWindow: 1_000_000,
	});
});

test("loads every official page once during the long-lived cache window", async () => {
	let requests = 0;
	const { config } = catalogServer((request) => {
		requests += 1;
		const page = new URL(request.url).searchParams.get("page");
		return page === "2"
			? catalogPage([modelCard("glm/glm-5", null)])
			: catalogPage([modelCard("cx/gpt-5.6-sol", "1M")], 2);
	});

	const first = await publicModelMetadata(config);
	const second = await publicModelMetadata(config);

	expect([...first.keys()]).toEqual(["cx/gpt-5.6-sol", "glm/glm-5"]);
	expect(second).toBe(first);
	expect(requests).toBe(2);
});

function catalogServer(handler: (request: Request) => string): { config: ProviderConfig } {
	const server = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		fetch: (request) => new Response(handler(request)),
	});
	servers.push(server);
	return {
		config: {
			baseUrl: "unused",
			requestTimeoutMs: 500,
			publicCatalogUrl: `http://127.0.0.1:${server.port}/models`,
			publicCatalogTtlMs: 86_400_000,
			publicCatalogTimeoutMs: 500,
		},
	};
}

function catalogPage(cards: readonly string[], pages = 1): string {
	const links = Array.from({ length: pages }, (_, index) => `<a href="?page=${index + 1}">`);
	const frame = JSON.stringify([1, cards.map((card) => `"card":${card}`).join(",")]);
	return `${links.join("")}<script>self.__next_f.push(${frame})</script>`;
}

function modelCard(id: string, context: string | null): string {
	return JSON.stringify({
		id,
		name: id === "cx/gpt-5.6-sol" ? "GPT-5.6 Sol" : "GLM-5",
		tagline: "Powerful agentic coding model",
		price: { free: false, label: "$$0.10", directional: false },
		context,
	});
}
