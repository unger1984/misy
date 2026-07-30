import { expect, test } from "bun:test";
import { referencePricing } from "../src/model-metadata";

test("resolves routed model prices and subscription-free catalogs", () => {
	expect(referencePricing("cx/gpt-5.6-sol-review")).toBe("$5/30");
	expect(referencePricing("ag/gemini-3-5-flash-agent")).toBe("$1.5/9");
	expect(referencePricing("gc/gemini-2.5-pro")).toBe("free");
	expect(referencePricing("kmc/k3")).toBe("free");
	expect(referencePricing("am/free")).toBe("free");
	expect(referencePricing("unknown/model")).toBeUndefined();
});
