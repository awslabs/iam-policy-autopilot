# Telemetry

IAM Policy Autopilot collects **anonymous, non-personally-identifiable** usage telemetry to help the development team understand how the tool is used, prioritize feature development, and measure adoption.

## What Is Collected

Telemetry records **only** which commands and parameters are used. It **never** collects file paths, file contents, AWS account IDs, AWS regions, credentials, policy content, or any personally identifiable information.

### CLI: `generate-policies` Command

| Parameter | What We Record |
|-----------|---------------|
| `source_files` | Whether provided (boolean), not the paths |
| `region` | Whether a non-default value was provided (boolean), not the value |
| `account` | Whether a non-default value was provided (boolean), not the value |
| `service_hints` | Values (e.g., "s3", "ec2") — AWS service names only |
| `pretty` | Value (true/false) |
| `language` | Value if provided (e.g., "python", "go") |
| `full_output` | Value (true/false) |
| `individual_policies` | Value (true/false) |
| `upload_policies` | Whether provided (boolean), not the prefix value |
| `minimal_policy_size` | Value (true/false) |
| `disable_cache` | Value (true/false) |
| `explain` | Filter patterns (e.g., "s3:*") |
| `debug` | **Not collected** |

### CLI: `fix-access-denied` Command

| Parameter | What We Record |
|-----------|---------------|
| `source` | Whether provided via argument vs stdin (boolean) |
| `yes` | Value (true/false) |

### CLI: `extract-sdk-calls` Command

| Parameter | What We Record |
|-----------|---------------|
| `source_files` | Whether provided (boolean), not the paths |
| `pretty` | Value (true/false) |
| `language` | Value if provided (e.g., "python", "go") |
| `full_output` | Value (true/false) |
| `service_hints` | Values (e.g., "s3", "ec2") — AWS service names only |

### CLI: `mcp-server` Command

| Parameter | What We Record |
|-----------|---------------|
| `transport` | Value (stdio/http) |
| `port` | Whether a non-default port was used (boolean), not the value |

### MCP Server Tools

| Tool | What We Record |
|------|---------------|
| `generate_application_policies` | Whether source_files, region, account, and service_hints were provided |
| `generate_policy_for_access_denied` | That the tool was invoked (boolean) |
| `fix_access_denied` | That the tool was invoked (boolean) |

### Additional Data

Every telemetry event also includes:

| Data | Description |
|------|-------------|
| Command name | Which CLI command or MCP tool was used (e.g., "generate-policies") |
| Tool version | The version of IAM Policy Autopilot (e.g., "0.1.4") |

## How Data Is Transmitted

Telemetry data is encoded as custom HTTP request headers (`X-Ipa-*`) on a lightweight HTTPS GET request to the AWS service reference endpoint (`https://servicereference.us-east-1.amazonaws.com`). This is the same endpoint that IAM Policy Autopilot already uses during policy generation to fetch AWS service metadata.

No additional endpoints or third-party services are contacted.

## How Data Is Stored

Telemetry data is received and aggregated server-side. IP addresses are available in server logs but are not associated with telemetry data and are subject to standard log retention.

## Data Retention

Server-side logs containing IP addresses are retained for a maximum of **3 years**, in accordance with standard AWS operational practices.

## How to Opt Out

Set the `IAM_POLICY_AUTOPILOT_TELEMETRY` environment variable to `0`:

```bash
# Disable for a single invocation
IAM_POLICY_AUTOPILOT_TELEMETRY=0 iam-policy-autopilot generate-policies ./src/app.py

# Disable for the current shell session
export IAM_POLICY_AUTOPILOT_TELEMETRY=0

# Disable permanently (add to your shell profile)
echo 'export IAM_POLICY_AUTOPILOT_TELEMETRY=0' >> ~/.bashrc

# Disable in CI/CD (GitHub Actions example)
env:
  IAM_POLICY_AUTOPILOT_TELEMETRY: "0"

# Disable for MCP server (in your mcp.json config)
{
  "mcpServers": {
    "iam-policy-autopilot": {
      "command": "iam-policy-autopilot",
      "args": ["mcp-server"],
      "env": {
        "IAM_POLICY_AUTOPILOT_TELEMETRY": "0"
      }
    }
  }
}
```

When the environment variable is unset, telemetry is **enabled by default** and a one-line notice is printed to stderr (CLI) or sent as an MCP notification (MCP server) on each invocation. Setting the variable to any value (including `"0"` or `"1"`) permanently silences the notice.

## Telemetry Notice

When telemetry is enabled by default (environment variable unset), you will see:

**CLI mode** (on stderr):
```
[telemetry] Anonymous usage metrics enabled. Set IAM_POLICY_AUTOPILOT_TELEMETRY=0 to disable. See TELEMETRY.md
```

**MCP server mode** (via MCP `notifications/message`):
```
Anonymous usage metrics are enabled. Set IAM_POLICY_AUTOPILOT_TELEMETRY=0 in your MCP server env config to disable.
```

The notice disappears once you explicitly set `IAM_POLICY_AUTOPILOT_TELEMETRY` to any value.
