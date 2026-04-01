# Telemetry

IAM Policy Autopilot collects **anonymous, non-personally-identifiable** usage telemetry to help the development team understand how the tool is used, prioritize feature development, and measure adoption.

## What Is Collected

Telemetry records **only** which commands and parameters are used, and whether the command succeeded. It **never** collects file paths, file contents, AWS account IDs, AWS regions, credentials, policy content, or any personally identifiable information.

<!-- BEGIN AUTO-GENERATED TELEMETRY TABLE -->

### CLI: `generate-policies` Command

| Parameter | What We Record |
|-----------|---------------|
| `source_files` | count of items |
| `pretty` | actual value (boolean) |
| `language` | value if provided, omitted otherwise |
| `full_output` | actual value (boolean) |
| `region` | whether non-default (boolean) |
| `account` | whether non-default (boolean) |
| `individual_policies` | actual value (boolean) |
| `upload_policies` | presence (boolean) |
| `minimal_policy_size` | actual value (boolean) |
| `disable_cache` | actual value (boolean) |
| `service_hints` | list of values if non-empty, omitted otherwise |
| `explain` | list of values if non-empty, omitted otherwise |
| `tf_dir` | presence (boolean) |
| `tf_files` | presence (boolean) |
| `tfvars` | presence (boolean) |
| `tfstate` | presence (boolean) |
| `explain_resources` | presence (boolean) |
| `debug` | not collected |

### CLI: `fix-access-denied` Command
| Parameter | What We Record |
|-----------|---------------|
| `source` | presence (boolean) |
| `yes` | actual value (boolean) |

### CLI: `mcp-server` Command

| Parameter | What We Record |
|-----------|---------------|
| `transport` | actual value (McpTransport) |
| `port` | not collected |

### MCP: `mcp-tool-generate-policies`

| Parameter | What We Record |
|-----------|---------------|
| `source_files` | count of items |
| `region` | presence (boolean) |
| `account` | presence (boolean) |
| `service_hints` | list of values if non-empty, omitted otherwise |
| `tf_dir` | presence (boolean) |
| `tf_files` | presence (boolean) |
| `tfstate` | presence (boolean) |
| `tfvars` | presence (boolean) |

### MCP: `mcp-tool-generate-policy-for-access-denied`

| Parameter | What We Record |
|-----------|---------------|
| `error_message` | presence (boolean) |

### MCP: `mcp-tool-fix-access-denied`

| Parameter | What We Record |
|-----------|---------------|
| `access_denied_fix_policy` | presence (boolean) |
| `error_message` | presence (boolean) |

<!-- END AUTO-GENERATED TELEMETRY TABLE -->

### Result Data (recorded after execution)

| Field | Type | Description |
|-------|------|-------------|
| `success` | boolean | Whether the command completed successfully |
| `num_policies_generated` | number | Number of policies generated |
| `runtime_ms` | number | Pipeline execution time in milliseconds |
| `detected_language` | string | Programming language detected from source files (e.g., "python", "go") |
| `services_used` | string[] | AWS services found in the source code (e.g., ["s3", "dynamodb"]) |

### Installation ID

A persistent UUID v4 is stored in `~/.iam-policy-autopilot/config.json` as `installationId`. This allows counting unique installations across invocations without identifying individual users. The file also stores your telemetry preference (`telemetryChoice`).

## How Data Is Transmitted

Telemetry data is serialized as JSON, signed with AWS SigV4, and sent via an HTTPS POST request to a dedicated Lambda Function URL endpoint. The request includes a `Content-Type: application/json` header and an `IAMPolicyAutopilot/{version}` User-Agent header.

Telemetry is emitted **after** command execution completes, so that result data (success/failure) can be included.

## How Data Is Stored

Telemetry data is received by an AWS Lambda function, validated against a strict schema, and emitted as CloudWatch Embedded Metric Format (EMF) metrics. IP addresses are **not** logged or stored by the telemetry handler.

## Data Retention

CloudWatch metrics are retained according to standard CloudWatch retention policies. Server-side Lambda execution logs are retained for a maximum of **3 years**, in accordance with standard AWS operational practices.

## How to Opt Out

### Option 1: CLI command (persistent)

```bash
# Disable telemetry (persisted to ~/.iam-policy-autopilot/config.json)
iam-policy-autopilot telemetry --disable

# Re-enable telemetry
iam-policy-autopilot telemetry --enable

# Check current status
iam-policy-autopilot telemetry --status
```

### Option 2: Environment variable (overrides config file)

```bash
# Disable for a single invocation
DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY=true iam-policy-autopilot generate-policies ./src/app.py

# Disable for the current shell session
export DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY=true

# Disable permanently (add to your shell profile)
echo 'export DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY=true' >> ~/.bashrc

# Disable in CI/CD (GitHub Actions example)
env:
  DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY: "true"

# Disable for MCP server (in your mcp.json config)
{
  "mcpServers": {
    "iam-policy-autopilot": {
      "command": "iam-policy-autopilot",
      "args": ["mcp-server"],
      "env": {
        "DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY": "true"
      }
    }
  }
}
```

### Precedence

1. **Environment variable** `DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY=true` → disabled (highest priority)
2. **Config file** `~/.iam-policy-autopilot/config.json` (`telemetryChoice` field)
3. **Default**: telemetry ON, notice shown

## Telemetry Notice

When telemetry is enabled by default (no explicit choice made), a notice is shown:

**CLI mode** (on stderr):
```
IAM Policy Autopilot will collect telemetry data on command usage starting at version 0.2.0 (unless opted out)

        Overview: We do not collect customer content and we anonymize the
                  telemetry we do collect. See the attached link for more
                  information on what data is collected, why, and how to
                  opt-out. Telemetry will NOT be collected for any version
                  prior to 0.2.0 - regardless of opt-in/out.

        Opt-out:  Run `iam-policy-autopilot telemetry --disable`
                  or set DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY=true

        Details:  https://github.com/awslabs/iam-policy-autopilot/blob/main/TELEMETRY.md
```

**MCP server mode** (via MCP `notifications/message` at `Notice` level):
The same notice content is sent as a logging notification after the MCP handshake completes.

The notice disappears once you make an explicit choice via `telemetry --enable`, `telemetry --disable`, or by setting `DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY=true`.
