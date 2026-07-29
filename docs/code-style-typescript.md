# TypeScript Code Style

## Table of Contents

- [Purpose](#purpose)
- [Required Checks](#required-checks)
- [Compiler Configuration](#compiler-configuration)
- [Formatting and Linting](#formatting-and-linting)
- [Module Layout](#module-layout)
- [Naming](#naming)
- [Types](#types)
- [External Input](#external-input)
- [Error Handling](#error-handling)
- [Async and Cancellation](#async-and-cancellation)
- [Documentation](#documentation)
- [Tests](#tests)
- [Upstream Sources](#upstream-sources)

## Purpose

Rules for provider plugin packages under `plugins/`. Read [Code Style](code-style.md) first — it
holds the language-agnostic rules on size, module boundaries, comments, and error handling. This
document adds what is specific to TypeScript and does not repeat what is already there.

The size rules apply here in full. A helper crammed onto a single 200-column line is not concise,
it is unreviewable, and the current provider package is the reason this sentence exists.

## Required Checks

Green before a commit that changes the package's TypeScript source, tests, configuration, generated
output, or consumed shared contract, from the affected package root. Documentation-only, Rust-only,
and unrelated provider changes do not require these commands. A shared SDK change also requires
the checks of providers that consume the changed surface.

```bash
bunx tsc --noEmit
bun test
bunx biome check .
```

## Compiler Configuration

`strict: true` is the **floor, not the ceiling** — it enables the eight flags that existed when it
was introduced, and several of the most valuable checks were added later as opt-in.

Every package's `tsconfig.json` enables, in addition to `strict`:

```jsonc
{
  "compilerOptions": {
    "strict": true,

    // Not covered by `strict` — enable all of these.
    "noUncheckedIndexedAccess": true,      // arr[i] is T | undefined, not T
    "noImplicitOverride": true,
    "noFallthroughCasesInSwitch": true,
    "noPropertyAccessFromIndexSignature": true,
    "noImplicitReturns": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "verbatimModuleSyntax": true,
    "isolatedModules": true
  }
}
```

`noUncheckedIndexedAccess` is the highest-value flag here: it is exactly the check that turns a
silent `undefined` from a parsed provider response into a compile error.

`exactOptionalPropertyTypes` is **deliberately not required**. It is the one strictness flag whose
cost is driven by third-party typings rather than by our own code. Enable it if the package's
dependencies cooperate; do not block on it.

## Formatting and Linting

Use [Biome](https://biomejs.dev) — one binary, formatter and linter together, no config sprawl.
This is also what the `oh-my-pi` reference uses.

`biome.jsonc` in each package, with:

- `linter.rules.preset: "recommended"`;
- `formatter.lineWidth: 100` — the same width as the Rust side, so both halves of the repository
  read alike;
- `formatter.lineEnding: "lf"`.

Do not disable `noExplicitAny` — see [Types](#types).

Disabling `complexity/useLiteralKeys` is permitted and expected: `noPropertyAccessFromIndexSignature`
requires bracket access on index signatures (JSON-RPC `params`), which `useLiteralKeys` flags — the
compiler flag wins. The `off` in `biome.jsonc` must carry a comment stating this — the `.jsonc`
extension is what allows the comment.

A `// biome-ignore` must name the rule and state the reason on the same comment.

## Module Layout

- One module, one responsibility. In a provider package that means, at minimum, separate modules
  for **authentication**, **model catalog**, and **wire-format translation**. One file holding all
  three is the shape this rule exists to prevent.
- The process entry point (`src/index.ts`) does transport and dispatch only: read messages, route
  to a handler, write replies. No protocol translation, no network calls, no parsing of provider
  payloads.
- Export a deliberate surface. Everything else stays module-private; do not export a symbol only
  because a test wants it — test through the public surface or restructure.
- Use `node:`-prefixed imports for Node built-ins.

## Naming

- `camelCase` for variables, functions, and methods.
- `PascalCase` for types, interfaces, enums, and classes.
- `SCREAMING_SNAKE_CASE` for module-level constants.
- File names in `kebab-case`, named after the module's primary export.
- Do not prefix interfaces with `I`, and do not suffix types with `Type`.
- Fields that mirror an external wire format keep the wire spelling (`access_token`,
  `chatgpt-account-id`) and are confined to the type that models that payload. Convert to internal
  naming at the boundary rather than letting wire spelling leak inward.

## Types

- **No `any` in exported signatures.** Use `unknown` and narrow.
- Prefer `type` aliases and discriminated unions over class hierarchies. Model illegal states out
  of existence.
- No non-null assertions (`!`) on values that come from outside the module. Narrow explicitly.
- No type assertions (`as T`) on unvalidated data — an assertion silences the compiler without
  making the claim true. See [External Input](#external-input).
- `readonly` on anything not intended to be mutated by the caller.
- Prefer `satisfies` over an annotation when you want inference to stay narrow.

## External Input

Everything crossing a process or network boundary is `unknown` until proven otherwise: JSON-RPC
params from the core, HTTP responses from the provider, environment variables, decoded tokens.

- Validate at the boundary, once, in one place. Return a typed value or a clear error.
- A cast is not validation. `payload as ModelsResponse` on an unparsed body is a bug that surfaces
  as `undefined is not a function` three frames away.
- Validation failures name the field and what was expected, not just "invalid response".
- Never log or echo a token, credential, or authorization header. Redact before it reaches any
  output stream — stdout is the protocol channel.

## Error Handling

- Throw `Error` (or a subclass), never a string or a plain object.
- Preserve the cause: `new Error("could not list models", { cause: error })`.
- Error messages state what failed and with which provider or endpoint. `Request failed (401)` is
  not enough on its own — say which request.
- Catch narrowly. `catch (error: unknown)` then narrow; do not assume `error instanceof Error`
  without checking.
- Never swallow. An intentionally ignored failure carries a comment with the reason.

## Async and Cancellation

- No floating promises. Every promise is awaited, returned, or explicitly handled — a rejected
  promise nobody awaits is a silent failure.
- Every outbound request carries a timeout **and** an `AbortSignal`. An HTTP call with neither is a
  hang the core cannot recover from.
- Clean up on every path. A server socket, a timer, or a pending promise created in a start
  function must be released in a `finally`, including the abandoned path where the caller never
  returns.
- Anything with a lifetime gets an explicit expiry: a pending OAuth session must not live for the
  lifetime of the process.
- Do not use `await` inside a loop when the work is independent; batch it. Do use sequencing when
  ordering matters, and say so.

## Documentation

- TSDoc (`/** ... */`) on every exported function, type, and constant: purpose, and which failures
  it produces.
- A module header comment explaining the module's responsibility and its place in the plugin.
- Same **why, not what** rule as everywhere else.

## Tests

- `bun test`, colocated in the package under `tests/`.
- **No real network and no real credentials, ever.** Provider behavior is tested against a local
  fake HTTP server.
- Test observable behavior through the public surface, not internals.
- Cover the boundary explicitly: malformed payloads, expired tokens, refresh-and-retry, abandoned
  authorization, timeouts, cancellation. These are the paths that break in production and the ones
  a happy-path test suite never touches.

## Upstream Sources

- [TypeScript TSConfig reference](https://www.typescriptlang.org/tsconfig/) — every compiler flag,
  including the strict-family membership of each.
- [TSConfig `strict`](https://www.typescriptlang.org/tsconfig/strict.html) — what `strict` does and
  does not cover.
- [typescript-eslint shared configurations](https://typescript-eslint.io/getting-started/) and
  [typed linting](https://typescript-eslint.io/getting-started/typed-linting/) — the rule sets to
  mirror if a package ever needs ESLint instead of Biome.
- [Biome](https://biomejs.dev) — formatter and linter configuration.

Where this document and an upstream guideline conflict, raise it rather than silently choosing.
