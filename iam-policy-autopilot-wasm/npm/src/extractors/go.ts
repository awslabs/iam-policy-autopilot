import type { SdkCall } from "./utils.js";
import { stripQuotes } from "./utils.js";

const GO_SDK_RE = /github\.com\/aws\/aws-sdk-go(?:-v2)?\/service\/([a-z0-9]+)/;

export function extractGo(root: any): SdkCall[] {
  const calls: SdkCall[] = [];
  const aliases: Record<string, string> = {};

  // Phase 1: Find AWS SDK service imports
  for (const m of root.findAll({
    rule: {
      kind: "import_spec",
      has: { field: "path", kind: "interpreted_string_literal" },
    },
  })) {
    const pathNode = m.field("path");
    if (!pathNode) continue;
    const importPath = stripQuotes(pathNode.text());
    const svcMatch = importPath.match(GO_SDK_RE);
    if (!svcMatch) continue;
    const aliasNode = m.field("name");
    const localName = aliasNode ? aliasNode.text() : svcMatch[1];
    aliases[localName] = svcMatch[1];
  }

  // Phase 2: Find PascalCase method calls (likely SDK operations)
  for (const m of root.findAll({
    rule: {
      kind: "call_expression",
      has: { field: "function", kind: "selector_expression" },
    },
  })) {
    const fnNode = m.field("function");
    if (!fnNode) continue;
    const fieldNode = fnNode.field("field");
    if (!fieldNode) continue;
    const methodName = fieldNode.text();
    // Only PascalCase methods are SDK operations
    if (methodName[0] !== methodName[0].toUpperCase()) continue;
    const svcs = [...new Set(Object.values(aliases))];
    if (svcs.length) {
      calls.push({ Name: methodName, PossibleServices: svcs });
    }
  }

  return calls;
}
