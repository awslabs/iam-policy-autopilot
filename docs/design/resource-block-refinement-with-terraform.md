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

Organizations using Infrastructure-as-Code (IaC) already declare their AWS resources in Terraform. When an application references `s3_client.get_object(Bucket="my-app-data-bucket")`, and a Terraform file declares `resource "aws_s3_bucket" "data" { bucket = "my-app-data-bucket" }`, the tool can bind the SDK call's resource ARN to the specific bucket rather than a wildcard.

### Goals

1. **Concrete resource ARNs** — Replace `${BucketName}`, `${TableName}`, etc. in generated ARN patterns with values extracted from Terraform configuration.
2. **State file support** — Optionally use `terraform.tfstate` for exact deployed ARNs (including account, region, partition) when available.
3. **Unified CLI** — Integrate Terraform support as optional flags (`--terraform-dir`, `--tfstate`) on the existing `generate-policies` command rather than a separate subcommand.
4. **Non-disruptive** — When `--terraform-dir` is not provided, behavior is identical to the existing pipeline.

### Non-Goals

- CloudFormation or CDK support (future work).
- Automatic source code discovery from Terraform compute resources (deferred; the user provides source files explicitly).
- Terraform plan/apply integration.

## 3. User Experience

### CLI Interface

```bash
# Existing usage (unchanged)
iam-policy-autopilot generate-policies handler.py --region us-east-1 --account 123456789012

# New: with Terraform resource binding
iam-policy-autopilot generate-policies handler.py \
    --terraform-dir ./infra \
    --region us-east-1 --account 123456789012

# New: with Terraform state for exact ARNs
iam-policy-autopilot generate-policies handler.py \
    --terraform-dir ./infra \
    --tfstate ./infra/terraform.tfstate \
    --region us-east-1 --account 123456789012
```

### Output

When Terraform bindings are active, the output includes a `ResourceBindingExplanations` section documenting where each concrete ARN came from:

```json
{
  "Policies": [...],
  "ResourceBindingExplanations": [
    {
      "Arn": "arn:aws:s3:::my-app-data-bucket",
      "Source": "HCL",
      "TerraformResourceType": "aws_s3_bucket",
      "TerraformResourceName": "data_bucket",
      "Location": "main.tf:5"
    }
  ]
}
```

## 4. Architecture

### Pipeline Flow

The Terraform integration extends the existing extract → enrich → generate pipeline with two additional phases:

```
┌─────────────────────── Extraction Phase ───────────────────────┐
│                                                                 │
│  Source Files (.py, .go, .ts, .js)                              │
│       │                                                         │
│       ▼                                                         │
│  ExtractionEngine → SdkMethodCall[]                             │
│                                                                 │
│  Terraform Files (.tf)  ──→  HCL Parser ──→ TerraformResource[] │
│  Variable Defaults      ──→  Variable Resolver                  │
│  terraform.tfstate      ──→  State Parser ──→ StateResourceMap  │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────── Enrichment Phase ───────────────────────┐
│                                                                 │
│  SdkMethodCall[] → EnrichmentEngine → EnrichedSdkMethodCall[]   │
│                                                                 │
│  TerraformResource[] ──→ Service Resolver ──→ (service, suffix) │
│  (service, suffix)   ──→ ARN Pattern Lookup (via SDF)           │
│  Naming attributes   ──→ Concrete ARN derivation                │
│  StateResourceMap    ──→ State ARN attachment                   │
│                           │                                     │
│                           ▼                                     │
│  EnrichedSdkMethodCall[] + ResolvedResourceMap                  │
│       │                                                         │
│       ▼                                                         │
│  ARN Substitution: replace ${BucketName} with concrete values   │
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

### Module Structure

```
extraction/terraform/
├── mod.rs                  # Shared types: TerraformResource, AttributeValue, TerraformParseResult
├── hcl_parser.rs           # Parses .tf files → resource blocks
├── variable_resolver.rs    # Resolves var.xxx from defaults + .tfvars
└── state_parser.rs         # Parses terraform.tfstate for deployed ARNs

enrichment/terraform/
├── mod.rs
├── service_resolver.rs     # Maps Terraform types → IAM services via names_data.hcl
└── resource_binder.rs      # TerraformResourceResolver: ARN derivation + substitution
```

## 5. Detailed Design

### 5.1 HCL Parsing (`extraction/terraform/hcl_parser.rs`)

Recursively discovers `.tf` files in the provided directory (skipping `.terraform/`), parses them with the `hcl-rs` crate, and extracts `resource` blocks whose type starts with `aws_`.

**Input:** Directory path
**Output:** `TerraformParseResult` containing a `HashMap<(resource_type, local_name), TerraformResource>`

Each `TerraformResource` captures:
- Resource type (e.g., `"aws_s3_bucket"`)
- Local name (e.g., `"data_bucket"`)
- Attributes as `AttributeValue` (either `Literal` or `Expression`)
- Source file path and line number

### 5.2 Variable Resolution (`extraction/terraform/variable_resolver.rs`)

Resolves `var.xxx` references in resource attributes by reading:
1. `variable` block defaults from `.tf` files
2. Overrides from `terraform.tfvars`
3. Overrides from `*.auto.tfvars`

Handles bare variable references (`var.bucket_name`) and string interpolation (`"${var.prefix}-bucket"`). Unresolvable expressions remain as `AttributeValue::Expression` and produce wildcard bindings.

### 5.3 State File Parsing (`extraction/terraform/state_parser.rs`)

Parses `terraform.tfstate` (v4 JSON format) and extracts AWS resource instances with their deployed ARNs. State-derived ARNs take precedence over HCL-derived ARNs because they represent the actual deployed infrastructure.

### 5.4 Terraform Service Resolver (`enrichment/terraform/service_resolver.rs`)

Maps Terraform resource types to IAM service names using the embedded `names_data.hcl` from the Terraform AWS provider. For example:
- `aws_s3_bucket` → `("s3", "bucket")`
- `aws_dynamodb_table` → `("dynamodb", "table")`
- `aws_lambda_function` → `("lambda", "function")`

Uses a three-tier resolution strategy:
1. **Exact match** — for expanded `actual` patterns like `aws_canonical_user_id`
2. **Regex fallback** — for complex patterns with lookaheads (e.g., CloudWatch vs. CloudWatch Logs)
3. **Longest prefix match** — for standard `correct` prefixes

### 5.5 Resource Binding (`enrichment/terraform/resource_binder.rs`)

The `TerraformResourceResolver` is the central coordinator. Its `from_directory()` factory method orchestrates:

1. Parse HCL files
2. Resolve variables
3. Parse state file (if provided)
4. For each AWS resource:
   a. Resolve to IAM service + resource type via service resolver
   b. Look up ARN patterns from the Service Definition File (SDF)
   c. Derive the naming attribute from ARN placeholders (e.g., `${BucketName}` → `bucket` attribute)
   d. Construct an HCL-derived ARN by substituting the naming attribute value
   e. Attach state-derived ARN if available (takes precedence)

#### ARN Substitution Algorithm

For each action's resource ARN pattern:

1. If state-derived full ARNs exist for the `(service, resource_type)` key → use them directly
2. Otherwise, substitute HCL-derived binding names into ARN pattern placeholders
3. Infrastructure placeholders (`${Partition}`, `${Region}`, `${Account}`) are preserved for the policy generation engine to resolve
4. For sub-resources (e.g., S3 objects with `${BucketName}/${ObjectName}`), fall back to the parent resource binding and append `/*`

### 5.6 Unified API (`api/generate_policies.rs`)

The `generate_policies()` function accepts optional `terraform_dir` and `tfstate_path` in its config. When `terraform_dir` is present:

```rust
// 1. Resolve Terraform resources
let resolver = TerraformResourceResolver::from_directory(
    terraform_dir, tfstate_path, loader
).await?;

// 2-4. Standard extraction + enrichment (unchanged)
let extracted = ExtractionEngine::extract(&source_files).await?;
let enriched = EnrichmentEngine::enrich(&extracted).await?;

// 5. Terraform ARN substitution
let bound = resolver.substitute_enriched_calls(&enriched);
let explanations = resolver.build_binding_explanations();

// 6. Policy generation (unchanged)
let policies = PolicyGenerationEngine::generate(&bound)?;
```

## 6. ARN Resolution Priority

| Priority | Source | Example | When Used |
|---|---|---|---|
| 1 | `terraform.tfstate` | `arn:aws:s3:::my-app-data-bucket` | `--tfstate` provided and resource found in state |
| 2 | HCL attributes + SDF patterns | `arn:${Partition}:s3:::my-app-data-bucket` | Resource has a resolvable naming attribute |
| 3 | Wildcard | `arn:${Partition}:s3:::*` | Expression couldn't be resolved |
| 4 | Default (no Terraform) | `arn:${Partition}:s3:::${BucketName}` | `--terraform-dir` not provided |

## 7. Supported Resource Types

Any AWS resource type recognized by the Terraform AWS provider's `names_data.hcl` is supported. The embedded data currently covers 300+ service entries. Common examples:

| Terraform Type | IAM Service | Resource Suffix |
|---|---|---|
| `aws_s3_bucket` | `s3` | `bucket` |
| `aws_dynamodb_table` | `dynamodb` | `table` |
| `aws_sqs_queue` | `sqs` | `queue` |
| `aws_lambda_function` | `lambda` | `function` |
| `aws_sns_topic` | `sns` | `topic` |
| `aws_kinesis_stream` | `kinesis` | `stream` |

## 8. Limitations

1. **`local.*` and `module.*` references** — Not resolved. These produce wildcard bindings.
2. **Complex expressions** — Function calls, conditionals, and `for_each` expressions are preserved as-is and produce wildcard bindings.
3. **Cross-module references** — Module compositions where resources reference outputs from other modules are not followed.
4. **Non-AWS providers** — Only `aws_*` resource types are processed. Other providers (GCP, Azure) are silently ignored.
5. **Data sources** — `data` blocks are not processed (deferred for future auto-discovery feature).

## 9. Testing

### Unit Tests
- HCL parser: 15 tests covering resource extraction, data block handling, expression preservation, line numbers, comment skipping
- Variable resolver: 12 tests covering defaults, tfvars overrides, interpolation, partial resolution
- State parser: 14 tests covering ARN extraction, version validation, data source skipping
- Service resolver: 20+ tests covering exact match, regex fallback, prefix matching
- Resource binder: 20+ tests covering ARN substitution, state precedence, sub-resource fallback, binding explanations

### Integration Tests
- 19 tests in `tests/terraform_integration.rs` using sample `.tf` and `.tfstate` fixtures
- Cover end-to-end: parsing → resolving → ARN substitution with mock service reference data

## 10. Future Work

1. **Auto-discovery of source files** — Parse `aws_lambda_function` `handler` attributes and `archive_file` `source_dir` to automatically discover application source code from `.tf` files (code preserved in `source_tracer.rs` for re-enablement).
2. **CloudFormation support** — Extend the same pattern to CloudFormation templates.
3. **Terraform plan output** — Parse `terraform plan -json` for pre-apply resource bindings.
4. **Cross-module resolution** — Follow `module` blocks to resolve resources across module boundaries.
