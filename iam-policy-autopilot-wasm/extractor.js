/**
 * SDK method call extractor using web-tree-sitter.
 *
 * Parses source code with the official tree-sitter WASM grammars and
 * extracts `$OBJ.$METHOD($$ARGS)` patterns — i.e. any attribute-access
 * followed by a call. The output matches the Rust `SdkMethodCall` JSON
 * schema (PascalCase keys) so it can be passed directly into the Rust
 * WASM `validateAndGeneratePolicies` function.
 */

// Language → tree-sitter grammar WASM file mapping (loaded from CDN).
const GRAMMAR_FILES = {
	python: "https://unpkg.com/tree-sitter-python@0.23.6/tree-sitter-python.wasm",
	javascript: "https://unpkg.com/tree-sitter-javascript@0.23.1/tree-sitter-javascript.wasm",
	typescript: "https://unpkg.com/tree-sitter-typescript@0.23.2/tree-sitter-typescript.wasm",
	go: "https://unpkg.com/tree-sitter-go@0.23.4/tree-sitter-go.wasm",
};

// File extension → language key
const EXT_TO_LANG = {
	py: "python",
	js: "javascript",
	mjs: "javascript",
	cjs: "javascript",
	ts: "typescript",
	tsx: "typescript",
	go: "go",
};

let Parser = null;
const loadedLanguages = {};

/**
 * Initialise web-tree-sitter. Call once before `extractSdkCalls`.
 *
 * @param {string} [treeSitterWasmUrl] - Optional URL to the tree-sitter.wasm
 *   runtime file. Defaults to "tree-sitter.wasm" in the same directory.
 */
export async function initExtractor(treeSitterWasmUrl) {
	if (Parser) return; // already initialised

	// web-tree-sitter's UMD build registers on globalThis.TreeSitter.
	// Load it as a classic script to avoid ESM/fs bundling issues.
	if (!globalThis.TreeSitter) {
		await new Promise((resolve, reject) => {
			const s = document.createElement("script");
			s.src = "https://unpkg.com/web-tree-sitter@0.24.7/tree-sitter.js";
			s.onload = resolve;
			s.onerror = reject;
			document.head.appendChild(s);
		});
	}

	const TS = globalThis.TreeSitter;
	await TS.init({
		locateFile: () => treeSitterWasmUrl || "https://unpkg.com/web-tree-sitter@0.24.7/tree-sitter.wasm",
	});
	Parser = new TS();
}

/**
 * Load (and cache) a tree-sitter language grammar.
 *
 * @param {string} lang - One of "python", "javascript", "typescript", "go".
 * @param {string} [baseUrl=""] - Base URL where grammar .wasm files are served.
 * @returns {Promise<Language>}
 */
async function loadLanguage(lang, baseUrl = "") {
	if (loadedLanguages[lang]) return loadedLanguages[lang];
	const url = GRAMMAR_FILES[lang];
	if (!url) throw new Error(`Unsupported language: ${lang}`);
	const TS = globalThis.TreeSitter;
	const language = await TS.Language.load(url);
	loadedLanguages[lang] = language;
	return language;
}

/**
 * Detect language from a filename extension.
 *
 * @param {string} filename
 * @returns {string} Language key (e.g. "python")
 */
export function detectLanguage(filename) {
	const ext = filename.split(".").pop()?.toLowerCase();
	const lang = EXT_TO_LANG[ext];
	if (!lang) throw new Error(`Cannot detect language from filename: ${filename}`);
	return lang;
}

/**
 * Extract AWS SDK method calls from source code.
 *
 * Walks the AST looking for the pattern `OBJ.METHOD(ARGS)` — any call
 * expression whose function is an attribute/member access. This mirrors
 * the `$OBJ.$METHOD($$ARGS)` ast-grep pattern used by the Rust extractors.
 *
 * @param {string} source - Source code string.
 * @param {string} language - Language key ("python" | "javascript" | "typescript" | "go").
 * @param {string} [grammarBaseUrl=""] - Base URL for grammar .wasm files.
 * @returns {Promise<Array<{Name: string, PossibleServices: string[]}>>}
 *   Array of SDK method call objects in PascalCase matching the Rust schema.
 */
// Method names that are SDK client constructors, not API operations.
// These are filtered out since they don't map to IAM actions.
const CONSTRUCTOR_METHODS = new Set(["client", "resource", "Session", "Client", "NewFromConfig", "New"]);

export async function extractSdkCalls(source, language, grammarBaseUrl = "") {
	if (!Parser) throw new Error("Call initExtractor() first");

	const lang = await loadLanguage(language, grammarBaseUrl);
	Parser.setLanguage(lang);
	const tree = Parser.parse(source);

	const calls = [];
	const visitor = (node) => {
		if (isMethodCall(node, language)) {
			const info = extractMethodCallInfo(node, language);
			if (info && !CONSTRUCTOR_METHODS.has(info.methodName)) {
				calls.push({
					Name: info.methodName,
					PossibleServices: [], // filled in by Rust SDK model validation
				});
			}
		}
		for (let i = 0; i < node.childCount; i++) {
			visitor(node.child(i));
		}
	};
	visitor(tree.rootNode);

	return calls;
}

// ---------------------------------------------------------------------------
// AST helpers — language-aware method call detection
// ---------------------------------------------------------------------------

/**
 * Check whether a tree-sitter node represents a method call
 * (attribute access as the callee of a call expression).
 */
function isMethodCall(node, language) {
	const type = node.type;

	if (language === "go") {
		// Go: call_expression whose function child is a selector_expression
		if (type !== "call_expression") return false;
		const fn_ = node.childForFieldName("function");
		return fn_?.type === "selector_expression";
	}

	// Python / JS / TS: call node whose function is attribute / member_expression
	if (type !== "call") return false;
	const fn_ = node.childForFieldName("function");
	if (!fn_) return false;
	return (
		fn_.type === "attribute" || // Python
		fn_.type === "member_expression" // JS/TS
	);
}

/**
 * Extract method name and receiver from a method-call AST node.
 */
function extractMethodCallInfo(node, language) {
	if (language === "go") {
		const sel = node.childForFieldName("function");
		const operand = sel?.childForFieldName("operand");
		const field = sel?.childForFieldName("field");
		if (!field) return null;
		return {
			methodName: field.text,
			receiver: operand?.text ?? null,
		};
	}

	// Python / JS / TS
	const fn_ = node.childForFieldName("function");
	if (!fn_) return null;

	let methodName, receiver;
	if (fn_.type === "attribute") {
		// Python: attribute node has object + attribute children
		methodName = fn_.childForFieldName("attribute")?.text;
		receiver = fn_.childForFieldName("object")?.text;
	} else if (fn_.type === "member_expression") {
		// JS/TS: member_expression has object + property children
		methodName = fn_.childForFieldName("property")?.text;
		receiver = fn_.childForFieldName("object")?.text;
	}

	if (!methodName) return null;
	return { methodName, receiver: receiver ?? null };
}
