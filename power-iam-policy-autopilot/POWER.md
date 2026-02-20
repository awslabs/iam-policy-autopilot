---
name: "iam-policy-autopilot-power"
displayName: "IAM Policy Autopilot"
description: "AWS IAM Policy Autopilot analyzes your application code locally to generate and deploy identity-based policies for application roles, enabling faster IAM policy creation and reducing access troubleshooting time"
keywords: ["IAM", "AWS", "policy", "policies", "identity", "identity-based", "permission", "access", "denial", "deny", "denied", "lambda"]
mcpServers: "iam-policy-autopilot-mcp"
author: "AWS"
---
# Overview

Generate and deploy IAM policies for application code with deterministic code analysis, and automatically troubleshoot access denied errors. This power provides access to MCP tools that generate identity-based policies for application roles based on inputted source code, deploy those generated policies, and attempt to troubleshoot any inputted access denied errors.

--- 

# Onboarding

Before using IAM Policy Autopilot, make sure that the following are done and configured:
## Onboarding Part 1: validate that the binary command `iam-policy-autopilot` works, AND create necessary command aliases
This Kiro Power's requires the use of the `iam-policy-autopilot@latest` package from `uv`/`uvx`; this is because this Kiro power's MCP server's configuration requires running `uvx iam-policy-autopilot@latest mcp-server`. Just for context: although users may have installed the `iam-policy-autopilot` binary in some other way, we specifically do not want to use those other binaries because we want `uv`/`uvx` to fetch the latest version of its managed `iam-policy-autopilot` binary. As such, do this pre-requisite step to setup and make sure `uvx iam-policy-autopilot@latest` works, *and also* (because `uv/uvx` shell environemnt variables need to get picked up by the Kiro IDE) that the `iam-policy-autopilot-mcp` MCP server associated with this kiro power is successfully enabled:

1. try running `uvx iam-policy-autopilot@latest --version`; it should return `iam-policy-autopilot: version-id`, where "version-id" is something like "0.1.0". For context, this command fetches the latest version of the `iam-policy-autopilot` binary from the `uv` server, and runs it. This command should return something like `iam-policy-autopilot: version-id`, where "version-id" is something like "0.1.0".
    - [remediation step 1] if this `uvx iam-policy-autopilot@latest --version` command fails specifically because `uvx` does not exist, then we can try to remediate this by installing that `uvx` command ourselves. To do this, run `curl -LsSf https://astral.sh/uv/install.sh | sh`; if the user does not have `curl`, run instead the command `wget -qO- https://astral.sh/uv/install.sh | sh`. Then, add `uvx` to path by runnning either `source $HOME/.local/bin/env` for sh/bash/zsh shells, or `source $HOME/.local/bin/env.fish` for fish shells. Finally, check that the installation has succeeded by running `uvx iam-policy-autopilot@latest --version` and seeing if it returns something like `iam-policy-autopilot: version-id`, where "version-id" is something like "0.1.0". If this check succeeds, proceed to "step 2". If this check has failed, FAIL THE ENTIRE INSTALLATION PROCESS and tell the user that `uvx iam-policy-autopilot@latest` has failed, and they must get this command working themselves, e.g. by installing `uvx` via the instructions in https://docs.astral.sh/uv/getting-started/installation/. 
2. Make sure that the associated MCP server `iam-policy-autopilot-mcp` is up and running. Check that MCP calls to that server work. If they do not work, even though `uvx iam-policy-autopilot@latest --version` works, this may be because the Kiro IDE needs to pick up the latest shell environment changes. If they do not work, try to remediate the environment variables for the Kiro IDE. Then, wait a few seconds and try to reload the MCP server itself, **not the power**, and see if it works. If this is still unsuccessful, warn the user that they may manually go to the MCP server tab and click "Retry" on the `iam-policy-autopilot` MCP server there, to try to get it to reconnect.


## Onboarding Part 2 (optional): validate that the `aws` bin command exists, and that AWS credentials are configured.
This onboarding part consists of two steps. These two steps are optional if the user just wants to generate IAM policies for their applications or access denial errors (e.g. with the `generate_application_policies` and `generate_policy_for_access_denied` MCP tools in this kiro power), as policy generation does not require the `aws` cli or aws credentials. However, if the user wants to deploy their policy fixes to their AWS account (e.g. with the `fix_access_denied` MCP tool in this Kiro power), this requires both the following two steps to be performed, as both the `aws` cli and the user's active AWS credentials are required.

1. First, check that calling `aws --version` in the CLI does not return a "command not found" exception; it should instead return something like this: `aws-cli/2.27.18 Python/3.13.3 Darwin/25.1.0 exe/x86_64`.
    - if this does not work, FINISH the kiro power onboarding process, but WARN the user that they will not be able to perform the above actions that require the `aws` command and configured credentials. Tell the user to follow the setup guide for the AWS CLI in https://docs.aws.amazon.com/cli/v1/userguide/cli-chap-install.html, and then configure credentials by telling them to look at this link: https://docs.aws.amazon.com/cli/v1/userguide/cli-configure-files.html.
2. Second, call `aws configure list` in the CLI returns a table like the following below, AND THAT the `access_key` and `secret_key` entries in the table have values that are set.
    - if this does not work, PROCEED with the kiro power onboarding process, but WARN the user that they need to configure aws credentials, by telling them to look at this link: https://docs.aws.amazon.com/cli/v1/userguide/cli-configure-files.html.
```
      Name                    Value             Type    Location
      ----                    -----             ----    --------
   profile                <not set>             None    None
access_key     ****************NIUM shared-credentials-file    
secret_key     ****************TYnY shared-credentials-file    
    region                us-west-2      config-file    ~/.aws/config
```

## If both onboarding parts 1 and 2 have succeeded, then onboarding is complete. If onboarding part 1 has succeeded but `aws configure list` in onboarding part 2 returned empty values for either the access key or secret key, then onboarding is complete, but warn the user that they must configure their aws credentials.


---
# Best Practices and Ideal Use Cases

To understand the best practices and use cases of this MCP server's tools, please read through ALL the instructions and use cases in the descriptions of the `generate_application_policies`, `generate_policy_for_access_denied`, and `fix_access_denied` tools in this IAM Policy Autopilot MCP server (`iam-policy-autopilot-mcp`). 

Specifically, there are certain cases when this MCP server excels:
- **generating IAM policies for a code file used in an AWS deployment (e.g. AWS Lambda function)**: the `generate_application_policies` tool in the `iam-policy-autopilot-mcp` MCP server does exactly this. Take a look through ALL the instructions and use cases for this tool, to better undstand how it is useful.
- **troubleshooting/resolving AWS IAM access denied errors**: the `generate_policy_for_access_denied` and `fix_access_denied` tools in the `iam-policy-autopilot-mcp` MCP servers can be used in combination to attempt to fix IAM access denied errors. Take a look through ALL the instructions and use cases for each of those tools, to better understand how they are useful. For instance: if the user gives you an AWS access denied error they saw and asks you to diagnose/resolve it, OR if the user asks you to test an AWS deployment and you see an access denied error when testing, then you can do the following:
    1. invoke the `generate_policy_for_access_denied` tool, passing in the access denied error you saw. Follow ALL the instructions in that tool. This tool should retun an IAM policy to you, which should contain an attempted fix for the access denied policy.
    2. Then call the `fix_access_denied` tool using both that IAM policy returned from the `generate_policy_for_access_denied` tool, as well as the original access denied error. This will deploy the IAM policy generated by the `generate_policy_for_access_denied` tool to the AWS account, in an attempt to fix the access denied error.

--- 
### License and Privacy Information

This power integrates with the `iam-policy-autopilot` MCP server (Apache 2.0 license).

This Kiro power and the `iam-policy-autopilot` MCP server do not collect any form of user or telemetry data.
