/**
 * IAM Policy Autopilot — Browser WASM Package
 *
 * Generates least-privilege IAM policies from source code entirely in the browser.
 * Uses the full Rust extraction + enrichment + generation pipeline compiled to
 * WebAssembly via Emscripten with JSPI for async service reference fetches.
 *
 * @example
 * ```ts
 * import { generatePolicies } from 'iam-policy-autopilot-wasm';
 *
 * const result = await generatePolicies({
 *   files: [{ filename: 'handler.py', content: sourceCode }],
 *   region: 'us-east-1',
 *   account: '123456789012',
 * });
 * console.log(result.Policies);
 * ```
 */

export interface FileInput {
  /** Filename used for language detection, e.g. "handler.py", "index.ts", "main.go" */
  filename: string;
  /** Full source code content */
  content: string;
}

export interface GenerateInput {
  /** Source files to analyze */
  files: FileInput[];
  /** AWS region for ARN generation (default: "*") */
  region?: string;
  /** AWS account ID for ARN generation (default: "*") */
  account?: string;
  /** Language override (auto-detected from filename if omitted) */
  language?: string;
}

export interface PolicyStatement {
  Effect: string;
  Action: string[];
  Resource: string[];
}

export interface Policy {
  Version: string;
  Statement: PolicyStatement[];
}

export interface GenerateResult {
  Policies: Policy[];
}

export interface GenerateError {
  error: string;
}

// Emscripten exports C functions with a leading underscore prefix (the standard
// C name-mangling convention for exported symbols). E.g., a Rust `#[no_mangle]`
// function `generate_policies_wasm` becomes `_generate_policies_wasm` in the
// Emscripten JS glue. Helper functions like malloc/free follow the same pattern.
type EmscriptenModule = {
  _generate_policies_wasm: (ptr: number) => number;
  _free_string: (ptr: number) => void;
  _malloc: (size: number) => number;
  _free: (ptr: number) => void;
  UTF8ToString: (ptr: number) => string;
  stringToUTF8: (str: string, ptr: number, maxBytes: number) => void;
  lengthBytesUTF8: (str: string) => number;
};

// Singleton module instance
let modulePromise: Promise<EmscriptenModule> | null = null;
let generateFn: ((ptr: number) => Promise<number>) | null = null;

/**
 * Check if the current browser supports the required WebAssembly features.
 * Returns an object indicating support status and any missing features.
 *
 * @example
 * ```ts
 * const support = checkBrowserSupport();
 * if (!support.supported) {
 *   console.error('Missing:', support.missing.join(', '));
 * }
 * ```
 */
export function checkBrowserSupport(): { supported: boolean; missing: string[] } {
  const missing: string[] = [];

  if (typeof WebAssembly === 'undefined') {
    missing.push('WebAssembly');
  } else {
    if (typeof (WebAssembly as any).promising !== 'function') {
      missing.push('WebAssembly.promising (JSPI) — requires Chrome/Edge 137+, Firefox 153+, or a browser with JSPI support');
    }
    if (typeof (WebAssembly as any).Suspending !== 'function') {
      missing.push('WebAssembly.Suspending (JSPI)');
    }
  }

  return { supported: missing.length === 0, missing };
}

/**
 * Options for configuring how the WASM module is loaded.
 */
export interface InitOptions {
  /**
   * Override the path/URL to the WASM binary and JS glue.
   * By default, files are resolved relative to this module's location.
   * Provide the URL to the directory containing iam_policy_autopilot.js and .wasm.
   */
  locateFile?: (filename: string) => string;
}

/**
 * Initialize the WASM module. Called automatically on first use,
 * but can be called explicitly to pre-load the module.
 */
export async function init(options?: InitOptions): Promise<void> {
  if (modulePromise) return;
  modulePromise = loadModule(options);
  try {
    await modulePromise;
  } catch (e) {
    // Reset so the next call can retry
    modulePromise = null;
    throw e;
  }
}

async function loadModule(options?: InitOptions): Promise<EmscriptenModule> {
  // Check JSPI support before attempting to load
  const support = checkBrowserSupport();
  if (!support.supported) {
    throw new Error(
      `Browser does not support required WebAssembly features: ${support.missing.join('; ')}`
    );
  }

  // Load the Emscripten glue JS. It uses a UMD pattern (var createModule = ...),
  // so we fetch it as text and evaluate it to get the factory function.
  // The consumer must ensure the glue JS is accessible at the locateFile path.
  const baseUrl = options?.locateFile
    ? '' // locateFile handles all resolution
    : new URL('./', import.meta.url).href;

  const locateFile = options?.locateFile ?? ((filename: string) => `${baseUrl}${filename}`);
  const glueUrl = locateFile('iam_policy_autopilot.js');

  const glueResponse = await fetch(glueUrl);
  if (!glueResponse.ok) {
    throw new Error(`Failed to load WASM glue JS from ${glueUrl}: ${glueResponse.status}`);
  }
  const glueText = await glueResponse.text();

  // Evaluate the glue to get createModule. The glue assigns to `var createModule`
  // and also does `module.exports = createModule` for CJS. We extract it via Function().
  // NOTE: This requires the CSP to allow 'unsafe-eval'. If your environment restricts
  // eval/Function, you must load the glue JS via a <script> tag instead.
  const createModule = new Function(`${glueText}; return createModule;`)();

  const Module = await createModule({ locateFile }) as EmscriptenModule;

  // Wrap the export with WebAssembly.promising() for JSPI support
  generateFn = (WebAssembly as any).promising(Module._generate_policies_wasm);

  return Module;
}

/**
 * Generate IAM policies from source code.
 *
 * Runs the full pipeline: AST extraction → service reference enrichment → policy generation.
 * The source code never leaves the browser. The only network call is to the public
 * AWS Service Reference endpoint for IAM action metadata.
 *
 * @param input - Source files and optional AWS context
 * @returns Generated IAM policies
 * @throws Error if the WASM module fails to load or generation encounters an unrecoverable error
 */
export async function generatePolicies(input: GenerateInput): Promise<GenerateResult> {
  await init();
  const Module = await modulePromise!;

  if (!generateFn) {
    throw new Error('WASM module loaded but generate function not available');
  }

  const inputJson = JSON.stringify({
    files: input.files,
    region: input.region ?? '*',
    account: input.account ?? '*',
    language: input.language,
  });

  // Allocate input string in WASM memory
  const inputLen = Module.lengthBytesUTF8(inputJson) + 1;
  const inputPtr = Module._malloc(inputLen);
  try {
    Module.stringToUTF8(inputJson, inputPtr, inputLen);

    // Call the Rust function (suspends via JSPI during service reference fetches)
    const resultPtr = await generateFn(inputPtr);

    // Read result
    const resultJson = Module.UTF8ToString(resultPtr);
    Module._free_string(resultPtr);

    const parsed = JSON.parse(resultJson);
    if (parsed.error) {
      throw new Error(parsed.error);
    }
    return parsed as GenerateResult;
  } finally {
    Module._free(inputPtr);
  }
}
