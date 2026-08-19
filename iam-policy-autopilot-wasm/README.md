# iam-policy-autopilot-wasm

WebAssembly build of IAM Policy Autopilot. Generates IAM policies scoped to the AWS
SDK calls found in your source code, entirely in the browser — source code never
leaves the client.

## For Consumers

### Installation

```bash
npm install @aws/iam-policy-autopilot
```

### Usage

```typescript
import { generatePolicies, checkBrowserSupport } from "@aws/iam-policy-autopilot";

// Verify browser compatibility
const support = checkBrowserSupport();
if (!support.supported) {
  console.error("Unsupported:", support.missing);
  // Fall back to manual policy authoring
}

// Generate policies
const result = await generatePolicies({
  files: [{ filename: "handler.py", content: sourceCode }],
  region: "us-east-1",
  account: "123456789012",
});

console.log(result.Policies);
```

### Asset Hosting

The package includes two large files that must be served as static assets:

- `iam_policy_autopilot.js` (~59KB) — Emscripten glue
- `iam_policy_autopilot.wasm` (~55MB uncompressed, ~8MB gzipped)

Copy these from `node_modules/@aws/iam-policy-autopilot/dist/` to your static assets
directory (e.g. `public/` in Vite). Then tell the module where to find them:

```typescript
import { init } from "@aws/iam-policy-autopilot";

await init({
  locateFile: (filename) => `/static/wasm/${filename}`,
});
```

### Browser Requirements

JSPI (JavaScript Promise Integration) must be available. JSPI is a phase-4
WebAssembly proposal shipped in all three major engines:

- **Chrome/Edge 137+** and other Chromium-based browsers (Brave, Arc, Opera 121+, etc.)
- **Firefox 153+**
- **Safari 27+** (Safari 26.x and earlier are not supported; verify iOS Safari
  availability before relying on it)

Use `checkBrowserSupport()` for runtime feature detection rather than
user-agent sniffing.

### CSP Requirements

- `script-src`: must allow `'unsafe-eval'` (for Emscripten glue loading)
- `connect-src`: must allow `https://servicereference.us-east-1.amazonaws.com`

### Supported Languages

Python, JavaScript, TypeScript, Go, Java.

---

## For Maintainers

### How It Works

The full Rust pipeline (AST extraction via ast-grep/tree-sitter + enrichment via AWS
Service Reference + policy generation) is compiled to `wasm32-unknown-emscripten` as a
static library, then linked by `emcc` to produce browser-ready JS glue + WASM binary.

Async HTTP calls (to fetch service reference data during enrichment) use **JSPI**
(JavaScript Promise Integration). When Rust calls `em_fetch_get_sync()`, the WASM stack
suspends, the browser performs a `fetch()`, and execution resumes with the result.

```
┌─ npm wrapper (TypeScript) ─────────────────────────────────┐
│  generatePolicies() → init module → call WASM → parse JSON │
└────────────────────────────────────────────────────────────-┘
        │                                    ▲
        ▼                                    │
┌─ Emscripten WASM (Rust) ──────────────────────────────────-┐
│  generate_policies_wasm()                                   │
│    → Extraction (ast-grep + tree-sitter, all grammars)      │
│    → Enrichment (service reference via em_fetch_get_sync)   │
│    → Policy Generation                                      │
│    → return JSON string                                     │
└─────────────────────────────────────────────────────────────┘
        │ JSPI suspend
        ▼
┌─ Browser ──────────────────────────────────────────────────-┐
│  fetch('https://servicereference...amazonaws.com/...')       │
└─────────────────────────────────────────────────────────────┘
```

### Build Prerequisites

- [emsdk](https://emscripten.org/docs/getting_started/downloads.html) (latest)
- `rustup target add wasm32-unknown-emscripten`
- Node.js 18+

### Build

```bash
source ~/emsdk/emsdk_env.sh
./build.sh
```

This runs three stages:

1. `cargo build` → static library (`.a`)
2. `emcc` link → `dist/iam_policy_autopilot.{js,wasm}`
3. Copy to `npm/dist/`, compile TypeScript wrapper

### Testing the WASM Boundary

`test/wasm-invoke.test.mjs` is an end-to-end smoke test for the boundary: it loads
the built npm artifact, initializes the Emscripten module, invokes
`generatePolicies()` over the C-string FFI, and asserts sane policy JSON comes back.
It is intentionally shallow — pipeline logic is covered by the Rust unit tests.

Run locally (requires a completed `./build.sh` first):

```bash
# From the repo root (Node 26+)
node --test iam-policy-autopilot-wasm/test/wasm-invoke.test.mjs

# On Node 24, JSPI is still behind a flag:
node --experimental-wasm-jspi --test iam-policy-autopilot-wasm/test/wasm-invoke.test.mjs
```

Requirements:

- **Node 26+** — the module suspends on service-reference fetches via JSPI
  (`WebAssembly.promising`/`Suspending`), which Node 26 enables by default.
  Node 24 also works but needs the `--experimental-wasm-jspi` flag
- **Network access** to `https://servicereference.us-east-1.amazonaws.com` for
  enrichment data
- `npm/dist/` populated by `./build.sh` — the test serves it over a loopback HTTP
  server because the wrapper fetches the glue JS and `.wasm` the same way a browser
  would (Node's `fetch` doesn't support `file://` URLs)

In CI, the test runs automatically as the `WASM boundary test` step of the
[WASM Build workflow](../.github/workflows/wasm.yml), right after the artifact is
built. It triggers on pushes and PRs touching the WASM crate, the policy-generation
crate, or the workspace manifests.

### Project Structure

```
iam-policy-autopilot-wasm/
├── Cargo.toml          # Rust crate (staticlib)
├── src/lib.rs          # Entry point: generate_policies_wasm() extern "C"
├── em_fetch.js         # Emscripten JS library: browser fetch() FFI
├── build.sh            # Full build pipeline
├── npm/                # Publishable npm package
│   ├── package.json
│   ├── src/index.ts    # TypeScript wrapper (generatePolicies API)
│   └── dist/           # Build output (gitignored)
├── test/               # WASM boundary test (node --test, see Testing section)
├── index.html          # Standalone test page (dev only)
└── serve.sh            # Dev server for index.html
```

### Key Technical Decisions

| Decision                                   | Why                                                               |
| ------------------------------------------ | ----------------------------------------------------------------- |
| Emscripten over wasm-bindgen               | tree-sitter requires libc; wasm32-unknown-unknown can't compile C |
| JSPI over Asyncify                         | Asyncify's wasm-opt pass fails on pre-compiled Rust .a files      |
| Manual `WebAssembly.promising()`           | emcc only auto-wraps `main`, not custom exports                   |
| `new Function()` for glue loading          | Emscripten UMD output is incompatible with ESM `import()`         |
| `cfg(target_arch = "wasm32")`              | Automatic platform detection, no feature flags needed             |
| `unsafe_code = "allow"` in this crate only | FFI boundary requires unsafe; rest of workspace is `deny`         |

### Binary Size

55MB includes all ~25 tree-sitter grammars. Only 5 are needed. Future work: configure
`ast-grep-language` to include only Python, Go, JavaScript, TypeScript, Java. Expected
reduction: ~60-70% of binary size.

### Why Not Asyncify?

Asyncify requires `wasm-opt` to instrument the binary post-compilation. When the input
is a pre-compiled Rust static library (`.a`), `wasm-opt` can't find the
`__asyncify_get_call_index` helper functions it needs. This is a fundamental
incompatibility between Asyncify and the "compile Rust separately, link with emcc"
workflow. JSPI avoids this entirely — it operates at the VM level with no compile-time
instrumentation.
