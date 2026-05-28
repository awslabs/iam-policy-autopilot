/**
 * Client-side AWS SDK call extractor using @ast-grep/wasm.
 *
 * Orchestrates language-specific extractors that use the same structural
 * pattern matching approach as the Rust extractors.
 */
import {
  initializeTreeSitter,
  registerDynamicLanguage,
  parse,
} from "@ast-grep/wasm";
import type { SdkCall } from "./extractors/utils.js";
import { extractPython } from "./extractors/python.js";
import { extractJavaScript } from "./extractors/javascript.js";
import { extractGo } from "./extractors/go.js";

export type { SdkCall } from "./extractors/utils.js";

/** Language names the policy engine understands. */
export type LanguageName = "python" | "javascript" | "typescript" | "go";

/**
 * Language support descriptor. The consumer creates these and passes them
 * to `generatePoliciesFromSource` / `extractSdkCalls`.
 */
export interface LanguageSupport {
  /** Language identifier */
  name: LanguageName;
  /** URL or pre-fetched ArrayBuffer of the tree-sitter grammar `.wasm` file */
  grammar: string | BufferSource;
}

let initPromise: Promise<void> | null = null;
const registeredLanguages = new Set<string>();

/** @internal — called by init() in index.ts */
export function _configureTreeSitter(_wasmSource: string | BufferSource): void {
  // ast-grep handles tree-sitter.wasm loading internally
}

async function ensureAstGrep(): Promise<void> {
  if (initPromise) return initPromise;
  initPromise = initializeTreeSitter();
  return initPromise;
}

function toBlobUrl(buf: BufferSource): string {
  const blob = new Blob([buf], { type: "application/wasm" });
  return URL.createObjectURL(blob);
}

async function ensureLanguage(lang: LanguageSupport): Promise<void> {
  await ensureAstGrep();
  if (registeredLanguages.has(lang.name)) return;

  const libraryPath =
    typeof lang.grammar === "string" ? lang.grammar : toBlobUrl(lang.grammar);

  const config: Record<string, unknown> = { libraryPath };
  // Python uses µ as the expando char for metavariables
  if (lang.name === "python") (config as any).expandoChar = "\u00B5";

  await registerDynamicLanguage({ [lang.name]: config } as any);
  registeredLanguages.add(lang.name);
}

const extractors: Record<LanguageName, (root: any) => SdkCall[]> = {
  python: extractPython,
  javascript: extractJavaScript,
  typescript: extractJavaScript,
  go: extractGo,
};

/**
 * Parse source code and extract AWS SDK method calls.
 *
 * @param source   - Source code to analyze
 * @param language - Language support descriptor (provides grammar URL or buffer)
 */
export async function extractSdkCalls(
  source: string,
  language: LanguageSupport,
): Promise<SdkCall[]> {
  await ensureLanguage(language);
  const sg = parse(language.name, source);
  const root = sg.root();
  const fn = extractors[language.name];
  if (!fn) throw new Error("No extractor for: " + language.name);
  return fn(root);
}
