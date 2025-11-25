# IAM Policy Autopilot

An open source Model Context Protocol (MCP) server and command-line tool that helps your AI coding assistants quickly create baseline IAM policies that you can refine as your application evolves, so you can build faster. IAM Policy Autopilot analyzes your application code locally to generate identity-based policies for application roles, enabling faster IAM policy creation and reducing access troubleshooting time.

## Who is IAM Policy Autopilot for?

IAM Policy Autopilot is for any builders on AWS using AI coding assistants, including developers, product managers, technical experimenters, and business leaders.

## How IAM Policy Autopilot is helpful?

IAM Policy Autopilot is:

### Fast

IAM Policy Autopilot accelerates development by generating baseline identity-based IAM policies. Your AI coding assistant can call IAM Policy Autopilot to analyze AWS SDK calls within your application. IAM Policy Autopilot then automatically creates the baseline IAM permissions for your application roles.

### Reliable

IAM Policy Autopilot's deterministic code analysis helps create reliable and valid IAM policies that reduce policy troubleshooting. By using valid policies created with the MCP server, you reduce time spent on policy-related debugging and accelerate application deployment by avoiding permission-related delays.

### Up-to-date

IAM Policy Autopilot stays up to date with the latest AWS services and features so that builders and coding assistants have access to the latest AWS IAM permissions knowledge. It helps keep your application role's permissions current with AWS's evolving capabilities.

## Getting Started

### Prerequisites

**Installation Requirements**

Python 3.8+ is supported.

### Installation

#### Option 1: Using uv (Recommended)

Install [uv](https://docs.astral.sh/uv/getting-started/installation/) from Astral or [Github ReadMe](https://github.com/astral-sh/uv#installation).

No additional installation needed - you can run IAM Policy Autopilot directly using `uvx iam-policy-autopilot`.

#### Option 2: Using pip

```bash
pip install iam-policy-autopilot
```

### AWS Configuration

IAM Policy Autopilot requires AWS credentials to apply policy fixes and upload policies. You can configure AWS credentials using one of the following methods:

#### Option 1: AWS CLI Configuration (Recommended)

Install and configure the AWS CLI:

```bash
# Install AWS CLI (if not already installed)
# macOS
brew install awscli

# Linux
curl "https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip" -o "awscliv2.zip"
unzip awscliv2.zip
sudo ./aws/install

# Configure AWS credentials
aws configure
```

This will prompt you for:
- AWS Access Key ID
- AWS Secret Access Key
- Default region name (e.g., `us-east-1`)
- Default output format (e.g., `json`)

#### Option 2: AWS Profiles

If you have multiple AWS accounts, you can use named profiles:

```bash
# Configure a named profile
aws configure --profile my-profile

# Use the profile in MCP configuration (see below)
```

For more information on AWS credential configuration, see the [AWS CLI Configuration Guide](https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-files.html).

### MCP Server Configuration

Configure the MCP server in your MCP client configuration to enable your AI coding assistant to generate IAM policies.

#### For Kiro

**If using uv/uvx:**

Add the following configuration to your project-level `.kiro/settings/mcp.json`:

```json
{
  "mcpServers": {
    "iam-policy-autopilot": {
      "command": "uvx",
      "args": ["iam-policy-autopilot", "mcp-server"],
      "env": {
        "AWS_PROFILE": "your-profile-name",
        "AWS_REGION": "us-east-1"
      },
      "disabled": false,
      "autoApprove": []
    }
  }
}
```

**If using pip:**

```json
{
  "mcpServers": {
    "iam-policy-autopilot": {
      "command": "iam-policy-autopilot",
      "args": ["mcp-server"],
      "env": {
        "AWS_PROFILE": "your-profile-name",
        "AWS_REGION": "us-east-1"
      },
      "disabled": false,
      "autoApprove": []
    }
  }
}
```

#### For Amazon Q Developer CLI

**If using uv/uvx:**

Add the MCP client configuration to your agent file at `~/.aws/amazonq/cli-agents/default.json`:

```json
{
  "mcpServers": {
    "iam-policy-autopilot": {
      "command": "uvx",
      "args": ["iam-policy-autopilot", "mcp-server"],
      "env": {
        "AWS_PROFILE": "your-profile-name",
        "AWS_REGION": "us-east-1"
      },
      "disabled": false,
      "autoApprove": []
    }
  },
  "tools": [
    "@iam-policy-autopilot"
  ]
}
```

**If using pip:**

```json
{
  "mcpServers": {
    "iam-policy-autopilot": {
      "command": "iam-policy-autopilot",
      "args": ["mcp-server"],
      "env": {
        "AWS_PROFILE": "your-profile-name",
        "AWS_REGION": "us-east-1"
      },
      "disabled": false,
      "autoApprove": []
    }
  },
  "tools": [
    "@iam-policy-autopilot"
  ]
}
```

#### For Claude Desktop

Add to your Claude Desktop configuration file:

**macOS**: `~/Library/Application Support/Claude/claude_desktop_config.json`

**Windows**: `%APPDATA%\Claude\claude_desktop_config.json`

**If using uv/uvx:**

```json
{
  "mcpServers": {
    "iam-policy-autopilot": {
      "command": "uvx",
      "args": ["iam-policy-autopilot", "mcp-server"],
      "env": {
        "AWS_PROFILE": "your-profile-name",
        "AWS_REGION": "us-east-1"
      }
    }
  }
}
```

**If using pip:**

```json
{
  "mcpServers": {
    "iam-policy-autopilot": {
      "command": "iam-policy-autopilot",
      "args": ["mcp-server"],
      "env": {
        "AWS_PROFILE": "your-profile-name",
        "AWS_REGION": "us-east-1"
      }
    }
  }
}
```

## CLI Usage

The `iam-policy-autopilot` CLI tool provides three main commands:

```
Generate IAM policies from source code and fix AccessDenied errors

Usage: iam-policy-autopilot <COMMAND>

Commands:
  fix-access-denied  Fix AccessDenied errors by analyzing and optionally applying IAM policy changes
  generate-policy    Generates complete IAM policy documents from source files
  mcp-server         Start MCP server
  help               Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help (see more with '--help')
  -V, --version  Print version
```

### Commands

**generate-policy** - Generates complete IAM policy documents from source files

```bash
iam-policy-autopilot generate-policy <source_files> [OPTIONS]
```

Example:

```bash
iam-policy-autopilot generate-policy \
  ./src/app.py \
  --region us-east-1 \
  --account 123456789012 \
  --pretty
```

Options:
- `--region <REGION>` - AWS region for resource ARNs
- `--account <ACCOUNT>` - AWS account ID for resource ARNs
- `--upload-policies <PREFIX>` - Upload generated policies to AWS IAM with the specified prefix
- `--pretty` - Pretty-print JSON output

**fix-access-denied** - Fix AccessDenied errors by analyzing and optionally applying IAM policy changes

```bash
iam-policy-autopilot fix-access-denied <access-denied-error-message> [OPTIONS]
```

Example:

```bash
iam-policy-autopilot fix-access-denied \
  "User: arn:aws:iam::123456789012:user/test is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/file.txt"
```

Options:
- `--yes` - Auto-apply policy changes without confirmation

**mcp-server** - Start MCP server locally

```bash
iam-policy-autopilot mcp-server [OPTIONS]
```

Options:
- `--transport <TRANSPORT>` - Transport type: `stdio` (default) or `http`

Example with HTTP transport:

```bash
# Start server at http://127.0.0.1:8001/mcp
iam-policy-autopilot mcp-server --transport http
```

**help** - Print help information

```bash
iam-policy-autopilot help [COMMAND]
```

### Global Options

- `-h, --help` - Print help information
- `-V, --version` - Print version

## Best Practices and Considerations

### Review and refine policies generated by IAM Policy Autopilot

IAM Policy Autopilot generates policies to provide a starting point that you can refine as your application matures. Review the generated policies so that they align with your security requirements before deploying them.

### Understand the IAM Policy Autopilot scope

IAM Policy Autopilot analyzes your application code to generate baseline IAM policies. It provides identity-based policies that grant permissions for AWS SDK operations detected in your code. Always review and scope down generated policies to implement least privilege access.

## Build Instructions

### Prerequisites

- [Rust](https://rustup.rs/) (latest stable version)
- Git

### Setup

Clone the repository with submodules:

```bash
git clone --recurse-submodules https://github.com/awslabs/iam-policy-autopilot.git
cd iam-policy-autopilot
```

If already cloned, initialize submodules:

```bash
git submodule init
git submodule update
```

Build the project:

```bash
cargo build --release
```

The compiled binary will be located at `target/release/iam-policy-autopilot`.

### Using the Built Binary with MCP

If you build from source, you can configure MCP clients to use the compiled binary:

```json
{
  "mcpServers": {
    "iam-policy-autopilot": {
      "command": "/path/to/iam-policy-autopilot",
      "args": ["mcp-server"]
    }
  }
}
```

To make the binary available system-wide:

```bash
# Copy the binary to /usr/local/bin
sudo cp ./target/release/iam-policy-autopilot /usr/local/bin/iam-policy-autopilot

# On macOS, sign the binary
sudo codesign -s - /usr/local/bin/iam-policy-autopilot
```

## Workspace Structure

This workspace contains several crates that work together:

- **`iam-policy-autopilot-policy-generation/`** - Core library providing SDK extraction and enrichment capabilities
- **`iam-policy-autopilot-access-denied/`** - Core library for parsing AccessDenied errors and synthesizing IAM policies
- **`iam-policy-autopilot-tools/`** - Policy upload utilities and AWS integration tools
- **`iam-policy-autopilot-cli/`** - Unified CLI tool providing all commands
- **`iam-policy-autopilot-mcp-server/`** - MCP server integration for IDE and tool integration

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
