import type { SdkCall } from "./utils.js";
import { stripQuotes } from "./utils.js";

const AWS_SDK_RE = /@aws-sdk\/(?:client|lib)-([a-z0-9-]+)/;
const COMMAND_RE = /^(.+)Command$/;
const PAGINATE_RE = /^paginate(.+)$/;
const WAITER_RE = /^waitUntil(.+)$/;

function parseImportNames(
  text: string,
  svc: string,
  svcMap: Record<string, string>,
  cmds: Set<string>,
): void {
  const inner = text.match(/\{([^}]*)\}/);
  const names = inner ? inner[1].split(",") : [text];
  for (const raw of names) {
    const trimmed = raw.trim();
    if (!trimmed) continue;
    const asParts = trimmed.split(/\s+as\s+/);
    const originalName = asParts[0].trim();
    const localName = (asParts[1] || asParts[0]).trim();
    svcMap[localName] = svc;
    if (COMMAND_RE.test(originalName)) cmds.add(localName);
  }
}

export function extractJavaScript(root: any): SdkCall[] {
  const calls: SdkCall[] = [];
  const svcMap: Record<string, string> = {};
  const cmds = new Set<string>();

  // Phase 1: Detect imports from @aws-sdk/client-* or @aws-sdk/lib-*
  for (const m of root.findAll("import $IMPORTS from $MODULE")) {
    const moduleNode = m.getMatch("MODULE");
    const importsNode = m.getMatch("IMPORTS");
    if (!moduleNode || !importsNode) continue;
    const modText = stripQuotes(moduleNode.text());
    const svcMatch = modText.match(AWS_SDK_RE);
    if (!svcMatch) continue;
    parseImportNames(importsNode.text(), svcMatch[1], svcMap, cmds);
  }

  // Also handle require() calls
  for (const m of root.findAll("require($MODULE)")) {
    const moduleNode = m.getMatch("MODULE");
    if (!moduleNode) continue;
    const modText = stripQuotes(moduleNode.text());
    const svcMatch = modText.match(AWS_SDK_RE);
    if (!svcMatch) continue;
    const declarator = m.parent_node();
    if (declarator && declarator.kind() === "variable_declarator") {
      const nameNode = declarator.field("name");
      if (nameNode) {
        parseImportNames(nameNode.text(), svcMatch[1], svcMap, cmds);
      }
    }
  }

  // Phase 2: Commands (e.g. GetObjectCommand → GetObject)
  for (const name of cmds) {
    const cm = name.match(COMMAND_RE);
    if (cm) calls.push({ Name: cm[1], PossibleServices: [svcMap[name]] });
  }

  // Phase 3: Paginators and waiters from imports
  for (const [name, svc] of Object.entries(svcMap)) {
    const pm = name.match(PAGINATE_RE);
    if (pm) {
      calls.push({ Name: pm[1], PossibleServices: [svc] });
      continue;
    }
    const wm = name.match(WAITER_RE);
    if (wm) calls.push({ Name: wm[1], PossibleServices: [svc] });
  }

  // Phase 4: client.send(new XxxCommand(...)) pattern via client variable tracking
  const clientVars: Record<string, string> = {};
  for (const m of root.findAll({ rule: { kind: "new_expression" } })) {
    const ctorNode = m.field("constructor");
    if (!ctorNode || !svcMap[ctorNode.text()]) continue;
    const parent = m.parent_node();
    if (parent && parent.kind() === "variable_declarator") {
      const nameNode = parent.field("name");
      if (nameNode) {
        clientVars[nameNode.text()] = svcMap[ctorNode.text()];
      }
    }
  }

  // Phase 5: client.methodName() shorthand calls
  for (const m of root.findAll({
    rule: {
      kind: "call_expression",
      has: { field: "function", kind: "member_expression" },
    },
  })) {
    const fnNode = m.field("function");
    if (!fnNode) continue;
    const objNode = fnNode.field("object");
    const propNode = fnNode.field("property");
    if (!objNode || !propNode) continue;
    const svc = clientVars[objNode.text()];
    if (!svc) continue;
    const opName = propNode.text().charAt(0).toUpperCase() + propNode.text().slice(1);
    if (!calls.some((c) => c.Name === opName && c.PossibleServices.includes(svc))) {
      calls.push({ Name: opName, PossibleServices: [svc] });
    }
  }

  return calls;
}
