# Emscripten WASM Build — Iteration Notes

## What We Proved

The **full Rust extraction pipeline** (ast-grep + tree-sitter + all language grammars + enrichment + policy generation) compiles successfully to `wasm32-unknown-emscripten`. This eliminates the need for a separate TypeScript extraction layer — single source of truth for SDK call extraction.

## Architecture

```
Browser JS                          Emscripten WASM (Rust)
┌──────────────────────┐           ┌──────────────────────────────────┐
│ index.html           │           │ iam-policy-autopilot-wasm-emscr. │
│   → createModule()   │──call──→  │   → generate_policies_wasm()     │
│   → _generate_...()  │           │     → Extraction (ast-grep)      │
│                      │           │     → Enrichment (service ref)   │
│                      │  ←fetch── │     → em_fetch_get_sync() [FFI]  │
│   browser fetch()    │──────→    │     → Policy Generation          │
│                      │           │   → return JSON                  │
└──────────────────────┘           └──────────────────────────────────┘
```

## Current Blocker: JSPI Suspend Error

```
Error: SuspendError: trying to suspend without WebAssembly.promising
```

### Root Cause

JSPI (JavaScript Promise Integration) requires that exported WASM functions which may suspend are wrapped with `WebAssembly.promising()`. Emscripten's `-sJSPI` flag is supposed to handle this automatically via `JSPI_EXPORTS`, but it's not working correctly — likely because:

1. The function call path from JS → `_generate_policies_wasm` → (deep call stack) → `em_fetch_get_sync` is too indirect for JSPI's static analysis to detect
2. JSPI is still experimental in Emscripten 5.0.7 and may have bugs with complex call graphs
3. The `tokio::runtime::Builder::new_current_thread().block_on()` in our entry point may interfere with JSPI's suspend mechanism

### What Needs to Happen Next

There are **three approaches** to resolve this, in order of preference:

#### Approach A: Use Asyncify Instead of JSPI (Recommended)

Asyncify is the mature, well-tested approach. The issue we hit earlier was that Asyncify needs to be enabled during the **Rust compilation step**, not just at link time.

**Steps:**

1. Add `-sASYNCIFY` to `EMCC_CFLAGS` during `cargo build`:
   ```bash
   EMCC_CFLAGS="-s ERROR_ON_UNDEFINED_SYMBOLS=0 --no-entry -sASYNCIFY" cargo build \
     --package iam-policy-autopilot-wasm-emscripten \
     --target wasm32-unknown-emscripten --release
   ```
2. Then link with:
   ```bash
   emcc ... -s ASYNCIFY -s 'ASYNCIFY_IMPORTS=["em_fetch_get_sync"]' ...
   ```
3. The key insight: Asyncify must be present in EMCC_CFLAGS during cargo build so the `.a` file contains the necessary instrumentation. We only tried it at link time before.

#### Approach B: Pre-fetch Service Reference Data in JS

Avoid the suspend problem entirely by fetching all service reference data in JavaScript before calling into WASM:

1. JS fetches `https://servicereference.us-east-1.amazonaws.com` to get the mapping
2. For each service found in the source code (detected via a lightweight pre-scan or passed by the user), JS fetches the individual service reference JSON
3. All data is passed into `generate_policies_wasm` as part of the input JSON
4. The Rust side uses pre-provided data instead of making HTTP calls

This avoids Asyncify/JSPI entirely but requires changes to the Rust API to accept pre-loaded service reference data.

#### Approach C: Fix JSPI Configuration

Debug why JSPI isn't wrapping the export correctly:

1. Build with `-sASSERTIONS=2` for better error messages
2. Check if `JSPI_EXPORTS=["generate_policies_wasm"]` (without underscore prefix) works
3. Try calling via `Module.ccall("generate_policies_wasm", "number", ["number"], [inputPtr], {async: true})` which explicitly requests async behavior
4. Check Chrome's `chrome://flags/#enable-experimental-webassembly-jspi` is enabled

## Build Commands Reference

### Prerequisites

```bash
# Install emsdk (one-time)
git clone https://github.com/emscripten-core/emsdk.git --depth 1 ~/emsdk
~/emsdk/emsdk install latest
~/emsdk/emsdk activate latest
source ~/emsdk/emsdk_env.sh

# Rust target
rustup target add wasm32-unknown-emscripten
```

### Build Steps

```bash
source ~/emsdk/emsdk_env.sh

# Step 1: Compile Rust to static library
EMCC_CFLAGS="-s ERROR_ON_UNDEFINED_SYMBOLS=0 --no-entry" cargo build \
  --package iam-policy-autopilot-wasm-emscripten \
  --target wasm32-unknown-emscripten --release

# Step 2: Link with emcc to produce JS glue + WASM
emcc target/wasm32-unknown-emscripten/release/libiam_policy_autopilot_wasm_emscripten.a \
  -o iam-policy-autopilot-wasm-emscripten/dist/iam_policy_autopilot.js \
  -s EXPORTED_FUNCTIONS='["_generate_policies_wasm","_free_string","_malloc","_free"]' \
  -s EXPORTED_RUNTIME_METHODS='["ccall","cwrap","UTF8ToString","stringToUTF8","lengthBytesUTF8"]' \
  -s MODULARIZE=1 \
  -s EXPORT_NAME="createModule" \
  -s ENVIRONMENT=web \
  -s ALLOW_MEMORY_GROWTH=1 \
  -s NO_EXIT_RUNTIME=1 \
  -s JSPI \
  -s JSPI_EXPORTS='["_generate_policies_wasm"]' \
  -s JSPI_IMPORTS='["em_fetch_get_sync"]' \
  -s ERROR_ON_UNDEFINED_SYMBOLS=0 \
  --js-library iam-policy-autopilot-wasm-emscripten/em_fetch.js \
  -O3 \
  --no-entry

# Step 3: Serve
python3 -m http.server 8081 -d iam-policy-autopilot-wasm-emscripten
```

### GitHub Actions

```yaml
- uses: mymindstorm/setup-emsdk@v14
  with:
    version: latest
- run: rustup target add wasm32-unknown-emscripten
- run: # ... build commands above
```

## Key Decisions Made

| Decision                                                  | Rationale                                                                    |
| --------------------------------------------------------- | ---------------------------------------------------------------------------- |
| `wasm32-unknown-emscripten` over `wasm32-unknown-unknown` | Only way to compile tree-sitter's C code (needs libc)                        |
| `staticlib` crate type                                    | Emscripten needs to do final linking to produce JS glue                      |
| Removed `reqwest` from wasm32 target                      | reqwest uses wasm-bindgen which is incompatible with emscripten              |
| `HttpGet` trait abstraction                               | Native uses reqwest, emscripten uses browser fetch via FFI                   |
| `em_fetch_get_sync` JS library                            | Provides browser fetch() to Rust via Emscripten's JS interop                 |
| JSPI over Asyncify (attempted)                            | Lighter weight, no compile-time instrumentation needed — but not working yet |

## File Layout

```
iam-policy-autopilot-wasm-emscripten/
├── Cargo.toml              # staticlib, depends on policy-generation with wasm+tree-sitter
├── src/lib.rs              # Entry point: generate_policies_wasm() extern "C"
├── em_fetch.js             # Emscripten JS library: browser fetch() FFI
├── index.html              # Test page
├── serve.sh                # Dev server
├── dist/                   # Build output (gitignored)
│   ├── iam_policy_autopilot.js    # Emscripten JS glue (~58KB)
│   └── iam_policy_autopilot.wasm  # Full WASM binary (~55MB uncompressed)
└── ITERATION-NOTES.md      # This file
```

## Binary Size Notes

The 55MB WASM binary includes ALL tree-sitter grammars from `ast-grep-language` (~25 languages). We only need 5 (Python, Go, JavaScript, TypeScript, Java). Future optimization: fork or configure `ast-grep-language` to only include needed grammars. Expected reduction: ~60-70% of binary size.

With gzip compression (standard for web serving), expect ~8-12MB transfer size.
