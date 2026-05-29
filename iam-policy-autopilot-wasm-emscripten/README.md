# iam-policy-autopilot-wasm-emscripten

Compiles the full IAM Policy Autopilot pipeline (AST extraction + enrichment + policy
generation) to WebAssembly via `wasm32-unknown-emscripten`. Produces a browser-ready npm
package that runs entirely client-side — source code never leaves the browser.

## Architecture

The Rust library (including ast-grep/tree-sitter for all supported languages) is compiled
to a static library, then linked by `emcc` to produce a JS glue file and `.wasm` binary.
Async HTTP calls to the AWS Service Reference endpoint use JSPI (JavaScript Promise
Integration) to suspend/resume the WASM stack during browser `fetch()`.

```
npm wrapper (generatePolicies)
  → loads Emscripten module
  → wraps export with WebAssembly.promising()
  → calls generate_policies_wasm(json_input)
      → Rust: AST extraction (ast-grep + tree-sitter)
      → Rust: Enrichment (service reference via em_fetch_get_sync FFI → browser fetch)
      → Rust: Policy generation
  → returns JSON policy document
```

## Browser Requirements

- **Chrome 131+** or Chromium-based browser (JSPI support required)
- Firefox and Safari are not currently supported (no JSPI)
- CSP must allow `unsafe-eval` (for loading the Emscripten glue JS via `new Function()`)
- CSP `connect-src` must allow `https://servicereference.us-east-1.amazonaws.com`

## Build

Prerequisites:

- [emsdk](https://emscripten.org/docs/getting_started/downloads.html) installed and activated
- `rustup target add wasm32-unknown-emscripten`
- Node.js (for TypeScript wrapper compilation)

```bash
source ~/emsdk/emsdk_env.sh
./build.sh
```

Output in `npm/dist/`:

- `iam_policy_autopilot.js` — Emscripten glue (~59KB)
- `iam_policy_autopilot.wasm` — WASM binary (~55MB uncompressed, ~8MB gzipped)
- `index.js` + `index.d.ts` — TypeScript wrapper with `generatePolicies()` API

## npm Package Usage

```typescript
import { generatePolicies, checkBrowserSupport } from "iam-policy-autopilot-wasm";

// Check browser compatibility first
const support = checkBrowserSupport();
if (!support.supported) {
  console.error("Unsupported browser:", support.missing);
}

// Generate policies from source code
const result = await generatePolicies({
  files: [{ filename: "handler.py", content: sourceCode }],
  region: "us-east-1",
  account: "123456789012",
});
console.log(result.Policies);
```

Consumers must serve the `.wasm` and glue `.js` files as static assets and pass
`locateFile` to tell the module where to find them:

```typescript
import { init } from "iam-policy-autopilot-wasm";

await init({
  locateFile: (filename) => `/static/wasm/${filename}`,
});
```

## Binary Size

The 55MB binary includes all ~25 tree-sitter grammars from `ast-grep-language`. Only 5
are needed (Python, Go, JavaScript, TypeScript, Java). Configuring `ast-grep-language` to
include only needed grammars would reduce the binary by ~60-70%. With gzip compression
(standard for web serving), current transfer size is ~8-12MB.

## Key Design Decisions

| Decision                                                  | Rationale                                                            |
| --------------------------------------------------------- | -------------------------------------------------------------------- |
| `wasm32-unknown-emscripten` over `wasm32-unknown-unknown` | tree-sitter requires libc (C code compilation)                       |
| JSPI over Asyncify                                        | Asyncify's wasm-opt pass fails on pre-compiled Rust static libraries |
| Manual `WebAssembly.promising()` wrapping                 | Emcc only auto-wraps `main`, not custom exports                      |
| `cfg(target_arch = "wasm32")` over feature flag           | Automatic platform detection, no consumer config needed              |
| `new Function()` for glue loading                         | Emscripten UMD output incompatible with ESM dynamic import           |
