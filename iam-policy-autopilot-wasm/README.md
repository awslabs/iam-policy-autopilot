# iam-policy-autopilot-wasm

WebAssembly build of [IAM Policy Autopilot](https://github.com/awslabs/iam-policy-autopilot) for generating least-privilege IAM policies entirely in the browser.

Source code is parsed client-side using [@ast-grep/wasm](https://www.npmjs.com/package/@ast-grep/wasm) (TypeScript extractors), then passed to the Rust enrichment and policy generation engine compiled to WASM. No backend service required — the customer's code never leaves the browser.

## Consumer Guide

### Installation

```bash
npm install iam-policy-autopilot-wasm
```

Tree-sitter grammars are optional peer dependencies — install only the languages you need:

```bash
npm install tree-sitter-python tree-sitter-javascript tree-sitter-typescript tree-sitter-go
```

### Usage

```ts
import { init, generatePolicies } from "iam-policy-autopilot-wasm";

// 1. Initialize once with WASM and grammar URLs
await init({
  policyEngineWasm: new URL("iam-policy-autopilot-wasm/dist/wasm/iam_policy_autopilot_wasm_bg.wasm", import.meta.url).href,
  treeSitterWasm: "", // handled internally by @ast-grep/wasm
  grammars: {
    python: new URL("tree-sitter-python/tree-sitter-python.wasm", import.meta.url).href,
    javascript: new URL("tree-sitter-javascript/tree-sitter-javascript.wasm", import.meta.url).href,
  },
});

// 2. Generate policies — language is auto-detected from filename
const result = await generatePolicies([{ filename: "handler.py", content: sourceCode }], { region: "us-east-1", account: "123456789012" });

console.log(JSON.stringify(result.Policies, null, 2));
```

### API

| Function                                                        | Description                                                             |
| --------------------------------------------------------------- | ----------------------------------------------------------------------- |
| `init(options)`                                                 | Initialize WASM modules and register grammars. Call once.               |
| `generatePolicies(files, options)`                              | Extract SDK calls from multiple files and generate a merged IAM policy. |
| `generatePoliciesFromSource(source, language, region, account)` | Single-file extraction + generation.                                    |
| `generatePoliciesFromCalls(calls, language, region, account)`   | Generate policies from pre-extracted SDK calls (skip extraction).       |
| `extractSdkCalls(source, language)`                             | Extract SDK calls only (no policy generation).                          |
| `detectLanguage(filename)`                                      | Detect language from file extension.                                    |

### Supported Languages

- Python (boto3 clients, paginators, waiters)
- JavaScript/TypeScript (@aws-sdk/client-\* commands, paginators, waiters)
- Go (aws-sdk-go-v2 service calls)

### Bundle Integration Notes

- The WASM binary (~19MB uncompressed, ~2.5MB gzipped) should be loaded lazily.
- `@ast-grep/wasm` is a runtime dependency (not bundled) — your bundler must resolve it.
- Grammar `.wasm` files are loaded at runtime via URL. Copy them to your `public/` directory or use `import.meta.url` resolution.
- For Vite: use `vite-plugin-wasm` and exclude `@ast-grep/wasm` from `optimizeDeps`.

---

## Maintainer Guide

### Architecture

The pipeline is split at the extraction boundary:

```
Browser (JS + @ast-grep/wasm)          Browser (Rust → WASM)
┌─────────────────────────────┐        ┌──────────────────────────────┐
│ Source Code                  │        │                              │
│   → @ast-grep/wasm (parse)  │        │  Enrichment Engine           │
│   → extractor.ts (patterns) │──JSON──│  (SDK model lookup)          │
│   → SDK calls [{Name, ...}] │        │  Policy Generation           │
└─────────────────────────────┘        │  (IAM policies)              │
                                       └──────────────────────────────┘
```

- **Extraction** runs in JavaScript using `@ast-grep/wasm` with the same pattern DSL as the native Rust extractors.
- **Enrichment + Policy Generation** runs in Rust compiled to `wasm32-unknown-unknown`, using embedded botocore service model data and the Service Reference endpoint for IAM action resolution.

### Project Structure

```
iam-policy-autopilot-wasm/
├── Cargo.toml          # Rust WASM crate (depends on policy-generation with default-features=false)
├── src/
│   ├── lib.rs          # WASM entry point (panic hook, re-exports)
│   └── policy.rs       # wasm-bindgen bindings: validateAndGeneratePolicies()
├── build.sh            # Single build script — runs all 4 stages
├── serve.sh            # Dev: build + serve index.html for local testing
├── index.html          # Standalone test console (no bundler needed)
├── extractor.js        # Standalone JS extractor for the test console
└── npm/
    ├── package.json    # npm package manifest
    ├── tsconfig.json   # TypeScript config for declaration emit
    ├── src/
    │   ├── index.ts        # Public API (init, generatePolicies, etc.)
    │   ├── extractor.ts    # @ast-grep/wasm orchestration
    │   └── extractors/
    │       ├── utils.ts    # SdkCall type, shared helpers
    │       ├── python.ts   # Python boto3 extractor
    │       ├── javascript.ts # JS/TS @aws-sdk extractor
    │       └── go.ts       # Go aws-sdk-go-v2 extractor
    └── dist/               # Build output (npm publish target)
        ├── index.js        # Bundled ESM entry point
        ├── extractor.js    # Bundled extractor (standalone use)
        ├── *.d.ts          # Type declarations
        └── wasm/           # wasm-bindgen output
            ├── iam_policy_autopilot_wasm_bg.wasm
            └── iam_policy_autopilot_wasm.js
```

### Build Pipeline

Run from the crate root:

```bash
bash build.sh
```

This executes four stages:

| Stage | Tool           | Input                                      | Output                                                 |
| ----- | -------------- | ------------------------------------------ | ------------------------------------------------------ |
| 1     | `cargo build`  | `src/lib.rs` + policy-generation crate     | Raw `.wasm` binary                                     |
| 2     | `wasm-bindgen` | Raw `.wasm`                                | JS glue + typed `.wasm` in `npm/dist/wasm/`            |
| 3     | `esbuild`      | `npm/src/index.ts`, `npm/src/extractor.ts` | Bundled ESM in `npm/dist/` (`@ast-grep/wasm` external) |
| 4     | `tsc`          | Same TS sources                            | `.d.ts` declaration files in `npm/dist/`               |

### Prerequisites

- Rust toolchain with `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- `wasm-bindgen-cli` matching the `wasm-bindgen` crate version: `cargo install wasm-bindgen-cli`
- Node.js (for esbuild + tsc): `npm install` in the `npm/` directory

### Local Development

For quick iteration on the test console (no npm package build):

```bash
bash serve.sh
# Opens http://localhost:8080/index.html
```

This uses `wasm-pack` to build the `pkg/` directory and serves the standalone `index.html` + `extractor.js` via Python's HTTP server.

### Key Design Decisions

- **`default-features = false`** on the policy-generation dependency disables `tree-sitter` (native C dep) and `telemetry` (tokio multi-thread) for WASM.
- **`rust-embed` without compression** on WASM — zstd has native C code that can't cross-compile to wasm32.
- **Release builds required** — `rust-embed` reads from filesystem in debug mode, which doesn't work in the browser.
- **`SystemTime` gated** — `std::time` is not implemented on `wasm32-unknown-unknown`. Caching uses simple in-memory maps without TTL on WASM.
- **`@ast-grep/wasm` is external** — not bundled into the package to avoid duplicating the 1.7MB binary when consumers already have it.
- **Tree-sitter grammars are optional peer deps** — loaded lazily per language at runtime so the base package stays small.

### Publishing

```bash
cd npm
npm publish
```

The `files` field in `package.json` ensures only `dist/` is published.
