import { expect, test } from "bun:test";
import { apiKeyCredentials, oauthCredentials } from "../src/index";

test("parses OAuth credentials and preserves provider-owned JSON fields", () => {
	expect(
		oauthCredentials({
			access_token: "token",
			extra: { tenant: "example" },
			type: "oauth",
		}),
	).toEqual({
		access_token: "token",
		extra: { tenant: "example" },
		type: "oauth",
	});
});

test("parses API-key credentials and rejects empty or foreign secrets", () => {
	expect(apiKeyCredentials({ api_key: "key", extra: 1, type: "api_key" })).toEqual({
		api_key: "key",
		extra: 1,
		type: "api_key",
	});
	expect(apiKeyCredentials({ api_key: "", type: "api_key" })).toBeUndefined();
	expect(apiKeyCredentials({ api_key: "  ", type: "api_key" })).toBeUndefined();
	expect(apiKeyCredentials({ access_token: "token", type: "oauth" })).toBeUndefined();
	expect(oauthCredentials({ api_key: "key", type: "api_key" })).toBeUndefined();
	expect(oauthCredentials({ access_token: "\t", type: "oauth" })).toBeUndefined();
});
