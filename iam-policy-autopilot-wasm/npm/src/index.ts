/**
 * IAM Policy Autopilot — WASM npm package
 *
 * Analyze source code and generate least-privilege IAM policies entirely
 * in the browser. No backend required.
 */
export { extractSdkCalls } from "./extractor.js";
export type { SdkCall, LanguageName, LanguageSupport } from "./extractor.js";
import { type LanguageName, type LanguageSupport, type SdkCall, extractSdkCalls, _configureTreeSitter } from "./extractor.js";

// The wasm-bindgen generated module is placed in dist/wasm/ by the build script.
// We import it dynamically to avoid bundling the WASM glue inline.
type WasmModule = {
  default: (input?: { module_or_path?: string | BufferSource }) => Promise<unknown>;
  validateAndGeneratePolicies: (json: string, lang: string, region: string, account: string) => Promise<string>;
};

let wasmModule: WasmModule | null = null;
let wasmInitPromise: Promise<void> | null = null;
let policyEngineWasmSource: string | BufferSource | null = null;

async function ensureWasm(): Promise<WasmModule> {
  if (wasmModule) return wasmModule;
  if (!wasmInitPromise) {
    wasmInitPromise = (async () => {
      // The wasm-bindgen JS glue is placed in dist/wasm/ by the build script.
      // Dynamic import path is relative to the output dist/ directory at runtime.
      const mod = await import("./wasm/iam_policy_autopilot_wasm.js") as unknown as WasmModule;
      await mod.default(
        policyEngineWasmSource ? { module_or_path: policyEngineWasmSource } : undefined,
      );
      wasmModule = mod;
    })();
  }
  await wasmInitPromise;
  return wasmModule!;
}

export interface InitOptions {
  /** URL or pre-fetched ArrayBuffer of the policy engine wasm binary. */
  policyEngineWasm: string | BufferSource;
  /** URL or pre-fetched ArrayBuffer of the web-tree-sitter runtime wasm. */
  treeSitterWasm: string | BufferSource;
  /** Grammar URLs or ArrayBuffers keyed by language name. */
  grammars?: Partial<Record<LanguageName, string | BufferSource>>;
}

const grammarRegistry: Partial<Record<LanguageName, string | BufferSource>> = {};

/**
 * Initialize the library. Call once before any other function.
 */
export async function init(options: InitOptions): Promise<void> {
  _configureTreeSitter(options.treeSitterWasm);
  policyEngineWasmSource = options.policyEngineWasm;
  if (options.grammars) {
    for (const [lang, grammar] of Object.entries(options.grammars)) {
      grammarRegistry[lang as LanguageName] = grammar;
    }
  }
  await ensureWasm();
}

const EXT_TO_LANGUAGE: Record<string, LanguageName> = {
  py: "python",
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  ts: "typescript",
  tsx: "typescript",
  mts: "typescript",
  cts: "typescript",
  go: "go",
};

/**
 * Detect language from a filename or path.
 * Returns undefined if the extension is not recognized.
 */
export function detectLanguage(filename: string): LanguageName | undefined {
  const ext = filename.split(".").pop()?.toLowerCase();
  return ext ? EXT_TO_LANGUAGE[ext] : undefined;
}

function resolveGrammar(lang: LanguageName): LanguageSupport {
  const grammar = grammarRegistry[lang];
  if (!grammar) {
    throw new Error(
      `No grammar registered for "${lang}". Pass it in init({ grammars: { ${lang}: ... } }).`,
    );
  }
  return { name: lang, grammar };
}

export interface PolicyResult {
  Policies: Array<{
    Policy: {
      Id: string;
      Version: string;
      Statement: Array<{
        Sid?: string;
        Effect: string;
        Action: string[];
        Resource: string[];
        Condition?: Record<string, Record<string, string[]>>;
      }>;
    };
    PolicyType: string;
  }>;
}

/**
 * Extract AWS SDK calls from source code and generate IAM policies.
 */
export async function generatePoliciesFromSource(
  source: string,
  language: LanguageSupport,
  region: string,
  account: string,
): Promise<PolicyResult> {
  const calls = await extractSdkCalls(source, language);
  if (calls.length === 0) return { Policies: [] };
  const wasm = await ensureWasm();
  const json = await wasm.validateAndGeneratePolicies(
    JSON.stringify(calls),
    language.name,
    region,
    account,
  );
  return JSON.parse(json);
}

/**
 * Generate IAM policies from pre-extracted SDK calls.
 */
export async function generatePoliciesFromCalls(
  calls: SdkCall[],
  language: string,
  region: string,
  account: string,
): Promise<PolicyResult> {
  if (calls.length === 0) return { Policies: [] };
  const wasm = await ensureWasm();
  const json = await wasm.validateAndGeneratePolicies(
    JSON.stringify(calls),
    language,
    region,
    account,
  );
  return JSON.parse(json);
}

export interface SourceFile {
  filename: string;
  content: string;
}

export interface GenerateOptions {
  region: string;
  account: string;
}

function dominantLanguage(langCounts: Record<string, number>): string {
  return Object.entries(langCounts).sort((a, b) => b[1] - a[1])[0][0];
}

/**
 * Extract SDK calls from multiple source files and generate a single merged IAM policy.
 */
export async function generatePolicies(
  files: SourceFile[],
  options: GenerateOptions,
): Promise<PolicyResult> {
  const allCalls: SdkCall[] = [];
  const langCounts: Record<string, number> = {};

  for (const file of files) {
    const lang = detectLanguage(file.filename);
    if (!lang) continue;
    const language = resolveGrammar(lang);
    const calls = await extractSdkCalls(file.content, language);
    for (const call of calls) {
      if (call.Name && typeof call.Name === "string" && Array.isArray(call.PossibleServices)) {
        const cleanServices = call.PossibleServices.filter(
          (s) => s != null && typeof s === "string",
        );
        if (cleanServices.length > 0) {
          allCalls.push({ Name: call.Name, PossibleServices: cleanServices });
          langCounts[lang] = (langCounts[lang] || 0) + 1;
        }
      }
    }
  }

  if (allCalls.length === 0) return { Policies: [] };

  const lang = dominantLanguage(langCounts);
  const wasm = await ensureWasm();
  const json = await wasm.validateAndGeneratePolicies(
    JSON.stringify(allCalls),
    lang,
    options.region,
    options.account,
  );
  return JSON.parse(json);
}
