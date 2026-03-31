# Resource Block Refinement with Terraform

## 1. Overview

IAM Policy Autopilot generates baseline IAM policies by analyzing application source code for AWS SDK calls. By default, policies use wildcard (`*`) resource ARNs because the tool has no knowledge of which specific AWS resources the application operates on.

This design extends the `generate-policies` command with optional Terraform integration that replaces wildcard ARNs with concrete resource identifiers extracted from Terraform HCL configuration files and/or `terraform.tfstate` state files. The result is more restrictive IAM policies that are closer to least-privilege.

### Example

**Without Terraform** (existing behavior):
```json
{
  "Effect": "Allow",
  "Action": "s3:GetObject",
  "Resource": "arn:aws:s3:::*/*"
}
```

**With Terraform** (new behavior):
```json
{
  "Effect": "Allow",
  "Action": "s3:GetObject",
  "Resource": "arn:aws:s3:::my-app-data-bucket/*"
}
```

## 2. Motivation

Customers using Infrastructure-as-Code (IaC) already declare their AWS resources in Terraform. When an application references `s3_client.get_object(Bucket="my-app-data-bucket")`, and a Terraform file declares `resource "aws_s3_bucket" "data" { bucket = "my-app-data-bucket" }`, the tool can bind the SDK call's resource ARN to the specific bucket rather than a wildcard.

### Goals

1. **Concrete resource ARNs** — Replace `${BucketName}`, `${TableName}`, etc. in generated ARN patterns with values extracted from Terraform configuration.
2. **State file support** — Optionally use `terraform.tfstate` for exact deployed ARNs (including account, region, partition) when available.
3. **Unified CLI** — Integrate Terraform support as optional flags (`--tf-dir`, `--tf-files`, `--tfvars`, `--tfstates`) on the existing `generate-policies` command rather than a separate subcommand.
4. **Resource binding explanations** — `--explain-resources` with ARN glob patterns shows where each concrete resource ARN came from.
5. **Non-disruptive** — When no Terraform flags are provided, behavior is identical to the existing pipeline.

### Non-Goals

- CloudFormation or CDK support (future work).
- Automatic source code discovery from Terraform compute resources (deferred; the user provides source files explicitly).
- Terraform plan/apply integration.

## 3. User Experience

### CLI Interface

```bash
# Existing usage (unchanged)
iam-policy-autopilot generate-policies handler.py --region us-east-1 --account 123456789012

# New: with Terraform directory (auto-discovers .tf and .tfvars)
iam-policy-autopilot generate-policies handler.py \
    --tf-dir ./infra \
    --region us-east-1 --account 123456789012

# New: with individual Terraform files + explicit tfvars
iam-policy-autopilot generate-policies handler.py \
    --tf-files ./infra/main.tf ./infra/variables.tf \
    --tfvars ./infra/terraform.tfvars \
    --region us-east-1 --account 123456789012

# New: with Terraform state for exact ARNs
iam-policy-autopilot generate-policies handler.py \
    --tf-dir ./infra \
    --tfstates ./infra/terraform.tfstate \
    --region us-east-1 --account 123456789012
```

### Output

When `--explain-resources` is provided with ARN glob patterns, the output includes a `ResourceBindingExplanations` section documenting where each matching concrete ARN came from:

```json
{
  "Policies": [...],
  "ResourceBindingExplanations": [
    {
      "Arn": "arn:aws:s3:::my-app-data-bucket",
      "Source": "Terraform",
      "ResourceType": "aws_s3_bucket",
      "ResourceName": "data_bucket",
      "Location": "main.tf:5.1-7.2"
    }
  ]
}
```

## 4. Architecture

### Resolution workflow

The Terraform integration extends the existing extract → enrich → generate pipeline with two additional phases:

```
┌──────────────── Terraform Resolution (optional) ───────────────┐
│                                                                 │
│  .tf files ──→ HCL Parser ──→ TerraformResources                │
│  .tfvars   ──→ Variable Resolver ──→ resolved attributes        │
│  .tfstate  ──→ State Parser ──→ TerraformStateResources         │
│                      │                                          │
│                      ▼                                          │
│  Service Resolver: aws_s3_bucket → (s3, bucket)                 │
│  ARN Pattern Lookup (via SDF): s3/bucket → arn patterns         │
│  Naming Attribute Derivation: ${BucketName} → "bucket" attr     │
│  State ARN Attachment: deployed ARN takes precedence             │
│                      │                                          │
│                      ▼                                          │
│  TerraformResourceResolver (ResolvedResourceMap)                │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────── Extraction Phase ───────────────────────┐
│                                                                 │
│  Source Files (.py, .go, .ts, .js)                              │
│       │                                                         │
│       ▼                                                         │
│  ExtractionEngine → SdkMethodCall[]                             │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────── Enrichment Phase ───────────────────────┐
│                                                                 │
│  SdkMethodCall[] → EnrichmentEngine → EnrichedSdkMethodCall[]   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌───────────────── ARN Substitution (optional) ──────────────────┐
│                                                                 │
│  EnrichedSdkMethodCall[] + TerraformResourceResolver            │
│       │                                                         │
│       ▼                                                         │
│  substitute_enriched_calls: replace ${BucketName} etc.          │
│  with concrete values from resolved Terraform resources         │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────── Policy Generation Phase ────────────────────┐
│                                                                 │
│  PolicyGenerationEngine → IAM Policy Documents                  │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

#### ARN Resolution Algorithm & Priority

For each action's resource ARN pattern:

1. If state-derived full ARNs exist for the `(service, resource_type)` key → use them directly
2. Otherwise, substitute HCL-derived binding names into ARN pattern placeholders
3. Infrastructure placeholders (`${Partition}`, `${Region}`, `${Account}`) are preserved for the policy generation engine to resolve
4. For sub-resources (e.g., S3 objects with `${BucketName}/${ObjectName}`), fall back to the parent resource binding and append `/*`

## Limitations

1. **`local.*` and `module.*` references** — Not resolved. These produce wildcard bindings.
2. **Complex expressions** — Function calls, conditionals, and `for_each` expressions are preserved as-is and produce wildcard bindings.
3. **Cross-module references** — Module compositions where resources reference outputs from other modules are not followed.
4. **Data sources** — `data` blocks are not processed (deferred for future auto-discovery feature).

## Future Work

1. **Auto-discovery of source files** — Parse `aws_lambda_function` `handler` attributes and `archive_file` `source_dir` to automatically discover application source code from `.tf` files (code preserved in `source_tracer.rs` for re-enablement).
2. **Terraform plan output** — Parse `terraform plan -json` for pre-apply resource bindings.
3. **Cross-module resolution** — Follow `module` blocks to resolve resources across module boundaries.
4. **Terraform provider** - Custom terraform provider to auto analyze source code and inject policies into .tf files.
