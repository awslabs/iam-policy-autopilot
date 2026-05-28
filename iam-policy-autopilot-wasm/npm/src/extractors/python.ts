import type { SdkCall } from "./utils.js";
import { stripQuotes } from "./utils.js";

// ast-grep uses µ as the expando char for Python (configured in registerDynamicLanguage)
const MU = "\u00B5";

function pyPattern(pattern: string, kind: string) {
  return { rule: { pattern, kind } };
}

export function extractPython(root: any): SdkCall[] {
  const calls: SdkCall[] = [];
  const clientVars: Record<string, string> = {};

  // Phase 1: Find boto3.client('service') / boto3.resource('service') assignments
  for (const m of root.findAll(
    pyPattern(`${MU}OBJ.${MU}METHOD(${MU}${MU}${MU}ARGS)`, "call"),
  )) {
    const obj = m.getMatch("OBJ");
    const method = m.getMatch("METHOD");
    if (!obj || !method) continue;
    if (obj.text() === "boto3" && (method.text() === "client" || method.text() === "resource")) {
      const args = m.getMultipleMatches("ARGS");
      const firstArg = args.find((a) => a.isNamed());
      if (!firstArg) continue;
      const svcName = stripQuotes(firstArg.text());
      const parent = m.parent_node();
      if (parent && parent.kind() === "assignment") {
        const lhs = parent.field("left");
        if (lhs) clientVars[lhs.text()] = svcName;
      }
    }
  }

  // Phase 2: Find method calls on known client variables
  for (const m of root.findAll(
    pyPattern(`${MU}OBJ.${MU}METHOD(${MU}${MU}${MU}ARGS)`, "call"),
  )) {
    const obj = m.getMatch("OBJ");
    const method = m.getMatch("METHOD");
    if (!obj || !method) continue;
    const methodName = method.text();
    if (obj.text() === "boto3") continue;
    if (["get_paginator", "get_waiter", "paginate", "wait"].includes(methodName)) continue;
    if (clientVars[obj.text()]) {
      calls.push({ Name: methodName, PossibleServices: [clientVars[obj.text()]] });
    }
  }

  // Phase 3: Paginators
  const paginatorVars: Record<string, { client: string; operation: string }> = {};
  for (const m of root.findAll(
    pyPattern(`${MU}CLIENT.get_paginator(${MU}NAME)`, "call"),
  )) {
    const client = m.getMatch("CLIENT")?.text();
    const name = stripQuotes(m.getMatch("NAME")?.text() ?? "");
    if (!client || !name) continue;
    const parent = m.parent_node();
    if (parent && parent.kind() === "assignment") {
      const lhs = parent.field("left");
      if (lhs) paginatorVars[lhs.text()] = { client, operation: name };
    }
  }
  for (const m of root.findAll(
    pyPattern(`${MU}PAGINATOR.paginate(${MU}${MU}${MU}ARGS)`, "call"),
  )) {
    const pagVar = m.getMatch("PAGINATOR")?.text();
    if (!pagVar) continue;
    const info = paginatorVars[pagVar];
    if (info && clientVars[info.client]) {
      calls.push({ Name: info.operation, PossibleServices: [clientVars[info.client]] });
    }
  }
  // Chained: client.get_paginator('op').paginate(...)
  for (const m of root.findAll(
    pyPattern(`${MU}CLIENT.get_paginator(${MU}NAME).paginate(${MU}${MU}${MU}ARGS)`, "call"),
  )) {
    const client = m.getMatch("CLIENT")?.text();
    const name = stripQuotes(m.getMatch("NAME")?.text() ?? "");
    if (client && name && clientVars[client]) {
      calls.push({ Name: name, PossibleServices: [clientVars[client]] });
    }
  }
  // Standalone paginator vars that weren't consumed by .paginate()
  for (const [, info] of Object.entries(paginatorVars)) {
    if (!calls.some((c) => c.Name === info.operation && c.PossibleServices.includes(clientVars[info.client] ?? ""))) {
      if (clientVars[info.client]) {
        calls.push({ Name: info.operation, PossibleServices: [clientVars[info.client]] });
      }
    }
  }

  // Phase 4: Waiters
  const waiterVars: Record<string, { client: string; waiterName: string }> = {};
  for (const m of root.findAll(
    pyPattern(`${MU}CLIENT.get_waiter(${MU}NAME)`, "call"),
  )) {
    const client = m.getMatch("CLIENT")?.text();
    const name = stripQuotes(m.getMatch("NAME")?.text() ?? "");
    if (!client || !name) continue;
    const parent = m.parent_node();
    if (parent && parent.kind() === "assignment") {
      const lhs = parent.field("left");
      if (lhs) waiterVars[lhs.text()] = { client, waiterName: name };
    }
  }
  for (const m of root.findAll(
    pyPattern(`${MU}WAITER.wait(${MU}${MU}${MU}ARGS)`, "call"),
  )) {
    const waitVar = m.getMatch("WAITER")?.text();
    if (!waitVar) continue;
    const info = waiterVars[waitVar];
    if (info && clientVars[info.client]) {
      calls.push({ Name: info.waiterName, PossibleServices: [clientVars[info.client]] });
    }
  }
  // Chained: client.get_waiter('name').wait(...)
  for (const m of root.findAll(
    pyPattern(`${MU}CLIENT.get_waiter(${MU}NAME).wait(${MU}${MU}${MU}ARGS)`, "call"),
  )) {
    const client = m.getMatch("CLIENT")?.text();
    const name = stripQuotes(m.getMatch("NAME")?.text() ?? "");
    if (client && name && clientVars[client]) {
      calls.push({ Name: name, PossibleServices: [clientVars[client]] });
    }
  }
  // Standalone waiter vars
  for (const [, info] of Object.entries(waiterVars)) {
    if (!calls.some((c) => c.Name === info.waiterName && c.PossibleServices.includes(clientVars[info.client] ?? ""))) {
      if (clientVars[info.client]) {
        calls.push({ Name: info.waiterName, PossibleServices: [clientVars[info.client]] });
      }
    }
  }

  return calls;
}
