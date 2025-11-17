# IAM Policy Autopilot

A unified toolset for AWS IAM policy management, providing both proactive policy generation from source code analysis and reactive error fixing from AccessDenied messages.

## Overview

This workspace contains the `iam-policy-autopilot` CLI tool, which provides three main commands:

1. **extract-sdk-calls** - Extract AWS SDK method calls from source code files for analysis
2. **generate-policy** - Generate complete IAM policies from source code with enrichment and validation
3. **fix-access-denied** - Parse and fix AccessDenied errors by analyzing error messages and synthesizing IAM policy changes

## Build Instructions

### Prerequisites
- Rust (latest stable version)
- Git

### Setup
1. Clone the repository with submodules:
   ```bash
   git clone --recurse-submodules <repository-url>
   ```

2. If already cloned, initialize submodules:
   ```bash
   git submodule init
   git submodule update
   ```

3. Build the project:
   ```bash
   cargo build
   ```

## Usage

### Extract SDK Calls from Source Code

Extract AWS SDK method calls from Python, JavaScript, TypeScript, or Go files:

```bash
cargo run -- extract-sdk-calls \
  iam-policy-autopilot/tests/resources/test_sample.py \
  --pretty --full-output
```

Example:
```bash
cargo run -- extract-sdk-calls \
  iam-policy-autopilot/tests/resources/test_sample.py \
  --language python --pretty
```

### Generate IAM Policies from Source Code

Generate complete IAM policies with enrichment from source code analysis:

```bash
cargo run -- generate-policy \
  iam-policy-autopilot/tests/resources/test_sample.py \
  --region us-east-1 \
  --account 123456789012 \
  --pretty
```

Example with policy upload:
```bash
cargo run -- generate-policy \
  iam-policy-autopilot/tests/resources/test_sample.py \
  --region us-east-1 \
  --account 123456789012 \
  --upload-policies CustomPolicyPrefix \
  --pretty
```

### Fix AccessDenied Errors

Parse and fix AWS AccessDenied errors by analyzing error messages:

```bash
# From command line argument
cargo run -- fix-access-denied \
  "User: arn:aws:iam::123456789012:user/test is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/file.txt"

# From stdin
echo "error message" | cargo run -- fix-access-denied

# Auto-apply without confirmation
cargo run -- fix-access-denied "error message" --yes
```

### Running the MCP Server
The MCP server is exposed through the subcommand `mcp-server`. 

#### Running with STDIO Transport 
For integrating the mcp binary with mcp clients using STDIO transport the binary needs to be available in the shell environment, for this we move our binary to `/usr/local/bin`

```
# Move the binary to /usr/local/bin
sudo cp ./target/debug/iam-policy-autopilot /usr/local/bin/iam-policy-autopilot
# If using MacOs, we need to sign the binary to run it under /usr/local/bin
sudo codesign -s - /usr/local/bin/iam-policy-autopilot
```

#### Running MCP with HTTP Transport

```
# Following command will start the server at `http://127.0.0.1:8001/mcp`
cargo run -- mcp-server --transport http
```

## Workspace Structure

This workspace contains several crates that work together:

- **`iam-policy-autopilot-policy-generation/`** - IAM Policy Autopilot core library providing SDK extraction and enrichment capabilities
- **`iam-policy-autopilot-access-denied/`** - Core library for parsing AccessDenied errors and synthesizing IAM policies
- **`iam-policy-autopilot-tools/`** - Policy upload utilities and AWS integration tools
- **`iam-policy-autopilot-cli/`** - Unified CLI tool providing all three commands (fix-access-denied, extract-sdk-calls, generate-policy)
- **`iam-policy-autopilot-mcp-server/`** - MCP server integration for IDE and tool integration

## Documentation

For detailed documentation, see the [Quick Start Guide](iam-policy-autopilot/README.md).

## Development

### Running Tests

```bash
# Run all tests
cargo test --workspace

# Run tests for specific crate
cargo test -p iam-policy-autopilot-cli
cargo test -p iam-policy-autopilot-access-denied
cargo test -p iam-policy-autopilot-policy-generation

# Run integration tests
cargo test -p iam-policy-autopilot-cli --test integration_tests
```

### Building Release Version

```bash
cargo build --release
```

The compiled binary will be located at `target/release/iam-policy-autopilot`.

## Security

See [CONTRIBUTING](CONTRIBUTING.md#security-issue-notifications) for more information.

## License

This project is licensed under the Apache-2.0 License.
