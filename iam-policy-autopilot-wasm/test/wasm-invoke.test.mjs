// End-to-end smoke test for the WASM boundary: built npm artifact → Emscripten
// module load → FFI invoke → sane policy JSON back.
//
// Scope is intentionally shallow — the Rust pipeline is covered by the crate's
// unit tests. This only validates packaging, module init, and the C-string FFI
// round trip.
//
// Requires: dist/ built by ../build.sh, Node >= 26 (JSPI on by default; on
// Node 24 pass --experimental-wasm-jspi — the module suspends on
// service-reference fetches via JSPI), and network access to the public AWS
// Service Reference endpoint.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { join, dirname, extname } from "node:path";
import { fileURLToPath } from "node:url";

const DIST_DIR = join(dirname(fileURLToPath(import.meta.url)), "..", "npm", "dist");
const MIME = { ".js": "text/javascript", ".wasm": "application/wasm" };

// The npm wrapper loads the glue JS and .wasm via fetch(), matching how a
// browser consumes it. Node's fetch does not support file:// URLs, so serve
// dist/ over a loopback HTTP server to exercise the same code path.
let server;
let baseUrl;

before(async () => {
  server = createServer(async (req, res) => {
    try {
      const file = await readFile(join(DIST_DIR, req.url.replace(/^\/+/, "")));
      res.writeHead(200, { "Content-Type": MIME[extname(req.url)] ?? "application/octet-stream" });
      res.end(file);
    } catch {
      res.writeHead(404).end();
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  baseUrl = `http://127.0.0.1:${server.address().port}/`;
});

after(() => server?.close());

test("wasm module loads and generates a policy for boto3 source", async () => {
  const { init, generatePolicies, checkBrowserSupport } = await import(new URL("index.js", `file://${DIST_DIR}/`).href);

  const support = checkBrowserSupport();
  assert.ok(support.supported, `JSPI not available (${support.missing.join("; ")}) — run node with --experimental-wasm-jspi`);

  await init({ locateFile: (filename) => `${baseUrl}${filename}` });

  const source = [
    "import boto3",
    "",
    "def handler(event, context):",
    "    s3 = boto3.client('s3')",
    "    s3.get_object(Bucket='my-bucket', Key='my-key')",
    "    s3.put_object(Bucket='my-bucket', Key='my-key', Body=b'data')",
    "",
  ].join("\n");

  const result = await generatePolicies({
    files: [{ filename: "handler.py", content: source }],
    region: "us-east-1",
    account: "123456789012",
  });

  assert.ok(result, "returns a result");
  assert.ok(Array.isArray(result.Policies), "result.Policies is an array");
  assert.ok(result.Policies.length > 0, "at least one policy generated");

  // Wire shape is PolicyWithMetadata: { Policy: { Version, Statement }, PolicyType }
  const statements = result.Policies.flatMap((p) => p.Policy?.Statement ?? []);
  assert.ok(statements.length > 0, "policy has statements");

  const actions = statements.flatMap((s) => s.Action ?? []);
  assert.ok(
    actions.some((a) => /^s3:GetObject$/i.test(a)),
    `expected s3:GetObject in actions, got: ${JSON.stringify(actions)}`,
  );
  assert.ok(
    actions.some((a) => /^s3:PutObject$/i.test(a)),
    `expected s3:PutObject in actions, got: ${JSON.stringify(actions)}`,
  );
  assert.ok(!actions.includes("*"), "no blanket wildcard action");
});
