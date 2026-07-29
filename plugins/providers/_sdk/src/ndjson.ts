/** Bounded newline framing for the provider SDK's untrusted stdin byte stream. */

const MAX_INPUT_FRAME_BYTES = 32 * 1024 * 1024;

/** One complete NDJSON line or an oversized-frame marker that carries no attacker input. */
export type InputFrame = { kind: "line"; line: string } | { kind: "too_large" };

/**
 * Splits arbitrary byte chunks into NDJSON frames without retaining more than one bounded frame.
 *
 * Oversized input is discarded through its next newline so a later valid request can still run.
 */
export class NdjsonFramer {
	private readonly decoder = new TextDecoder();
	private parts: Uint8Array[] = [];
	private byteLength = 0;
	private oversized = false;

	/** Consumes a transport chunk and returns every complete frame it contains. */
	push(chunk: Uint8Array): InputFrame[] {
		const frames: InputFrame[] = [];
		let start = 0;
		for (let index = 0; index < chunk.byteLength; index += 1) {
			if (chunk[index] !== 0x0a) continue;
			this.append(chunk.subarray(start, index));
			frames.push(this.endLine());
			start = index + 1;
		}
		this.append(chunk.subarray(start));
		return frames;
	}

	/** Reports a final oversized unterminated frame; valid unterminated input stays ignored. */
	finish(): InputFrame | undefined {
		return this.oversized ? this.take() : undefined;
	}

	private append(part: Uint8Array): void {
		if (this.oversized || part.byteLength === 0) return;
		this.byteLength += part.byteLength;
		if (this.byteLength > MAX_INPUT_FRAME_BYTES) {
			this.parts = [];
			this.oversized = true;
			return;
		}
		this.parts.push(part.slice());
	}

	private take(): InputFrame {
		const frame = this.oversized
			? { kind: "too_large" as const }
			: { kind: "line" as const, line: this.decode() };
		this.parts = [];
		this.byteLength = 0;
		this.oversized = false;
		return frame;
	}

	private endLine(): InputFrame {
		if (!this.oversized && this.byteLength + 1 > MAX_INPUT_FRAME_BYTES) {
			this.parts = [];
			this.oversized = true;
		}
		return this.take();
	}

	private decode(): string {
		const bytes = new Uint8Array(this.byteLength);
		let offset = 0;
		for (const part of this.parts) {
			bytes.set(part, offset);
			offset += part.byteLength;
		}
		return this.decoder.decode(bytes);
	}
}
