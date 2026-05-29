# Emscripten WASM Build — Architecture Notes

## Overview

The full Rust extraction pipeline (ast-grep + tree-sitter + all language grammars +
enrichment + policy generation) compiles to `wasm32-unknown-emscripten`. This eliminates
the need for a separate TypeScript extraction layer — single source of truth for SDK
call extraction.

## Architecture

```
Browser JS                          Emscripten WASM (Rust)
┌──────────────────────┐           ┌──────────────────────────────────┐
│ npm wrapper           │           │ iam-policy-autopilot-wasm-emscr. │
│   → generatePolicies()│──call──→  │   → generate_policies_wasm()     │
│   → WebAssembly       │           │     → Extraction (ast-grep)      │
│     .promising()      │           │     → Enrichment (service ref)   │
│                       │  ←fetch── │     → em_fetch_get_sync() [FFI]  │
│   browser fetch()     │──────→    │     → Policy Generation          │
│                       │           │   → return JSON                  │
└──────────────────────┘           └──────────────────────────────────┘
```

## How Async Works (JSPI)

The Rust code calls `em_fetch_get_sync()` as a normal blocking FFI function. In the
browser, this is an async `fetch()`. JSPI (JavaScript Promise Integration) bridges the
gap:

1. Emcc wraps `em_fetch_get_sync` with `WebAssembly.Suspending()` (via `-sJSPI_IMPORTS`)
2. We wrap `_generate_policies_wasm` with `WebAssembly.promising()` manually in JS
3. When Rust calls the FFI function, the WASM stack suspends, browser does the fetch,
   then resumes execution with the result

No Asyncify (which failed due to `__asyncify_get_call_index` incompatibility with
pre-compiled Rust static libraries). No code size bloat. JSPI is experimental but
shipping in Chrome 131+.

## Build Pipeline

```
Stage 1: cargo build --target wasm32-unknown-emscripten --release
          → .a static library

Stage 2: emcc link with JSPI flags + em_fetch.js library
          → dist/iam_policy_autopilot.js (59KB glue)
          → dist/iam_policy_autopilot.wasm (55MB uncompressed)

Stage 3: Copy artifacts to npm/dist/, compile TS wrapper
          → npm/dist/ ready for publishing
```

## Key Design Decisions

| Decision                                                  | Rationale                                                    |
| --------------------------------------------------------- | ------------------------------------------------------------ |
| `wasm32-unknown-emscripten` over `wasm32-unknown-unknown` | Only way to compile tree-sitter's C code (needs libc)        |
| `staticlib` crate type                                    | Emscripten needs to do final linking to produce JS glue      |
| Removed `reqwest` from wasm32 target                      | reqwest uses wasm-bindgen, incompatible with emscripten      |
| `HttpGet` trait abstraction                               | Native uses reqwest, emscripten uses browser fetch via FFI   |
| JSPI over Asyncify                                        | Asyncify's wasm-opt pass fails on pre-compiled Rust .a files |
| Manual `WebAssembly.promising()` wrapping                 | Emcc only auto-wraps `main`, not custom exports              |
| `cfg(target_arch = "wasm32")` over feature flag           | Automatic detection, no consumer configuration needed        |
| `new Function()` to load glue JS                          | Emscripten UMD output incompatible with ESM dynamic import   |

## Binary Size

55MB includes ALL tree-sitter grammars (~25 languages). Only 5 are needed (Python, Go,
JavaScript, TypeScript, Java). Future optimization: configure `ast-grep-language` to
include only needed grammars. Expected reduction: ~60-70%.

With gzip: ~8-12MB transfer size.

## Consuming in Vite/React

WASM files go in `public/` and are served as static assets. The npm wrapper uses
`locateFile` to tell Emscripten where to find the `.wasm` binary. In Vite dev,
`import.meta.url` resolves to the dev server origin. In production (CDN), it resolves
to the CDN origin. No conditional logic needed.

Key Vite config:

- `optimizeDeps.exclude: ['iam-policy-autopilot-wasm']`
- `server.fs.allow: ['..']` (for file: linked packages)
- WASM files in `public/` (copied via postinstall script)
