# Terraform model artifacts

These two committed JSON files drive the **Terraform plan → IAM policy** feature:
they let `iam-policy-autopilot` derive the AWS SDK operations (and thus IAM
actions) a `terraform apply`/`destroy` will perform, without running Terraform
or AWS. Both are embedded into the binary at build time (`rust-embed`) and
consumed by `src/extraction/terraform/plan_to_calls/`.

They are regenerated from the pinned `terraform-provider-aws` submodule in this
directory (currently `b53a72bc2e0`, tag `v6.34.0`) — see **Regeneration** below.

## `terraform-crud-map.json`

`resource_type` → its CRUD handler symbols, plus optional tagging entry points.
One array entry per managed resource (1240 resources; 617 are tag-managed).

```jsonc
{
  "resource_type": "aws_s3_bucket",
  "create_without_timeout": ".../internal/service/s3.resourceBucketCreate",
  "read_without_timeout":   ".../internal/service/s3.resourceBucketRead",
  "update_without_timeout": ".../internal/service/s3.resourceBucketUpdate",
  "delete_without_timeout": ".../internal/service/s3.resourceBucketDelete",
  "tags": {                                  // present only for @Tags resources
    "resource_type": "Bucket",               //   the @Tags resourceType (switch-arm key)
    "identifier_attribute": "bucket",        //   the @Tags identifierAttribute
    "list_tags_symbol":   ".../service/s3.(*servicePackage).ListTags",
    "update_tags_symbol": ".../service/s3.(*servicePackage).UpdateTags"
  }
}
```

- The four `*_without_timeout` values are full Go import-path symbols. A slot is
  omitted when the provider has no handler for it (e.g. `update` on an immutable
  resource — ~219 resources).
- **`tags`** is present only when the resource carries the provider's `@Tags`
  annotation **and** its service package implements the tagging interface. The
  provider reads/writes tags through a generated *interceptor* that wraps the
  CRUD handlers (calling the service package's `ListTags`/`UpdateTags`), so the
  tag SDK calls are **not** reachable from the CRUD handler bodies. Recording
  these symbols lets the model capture tag ops (e.g. `s3:GetBucketTagging`);
  without them, tag actions were missing from read/destroy policies.
  - `tags` carries **references only** (symbols); the SDK operations live in
    `terraform-model.json`, keyed by these symbols — never duplicated here.
  - `resource_type` is set only for the few services whose `ListTags` switches
    on resourceType (S3, IAM, …); it selects the arm.

## `terraform-model.json`

An `ExternalLibraryModel` (same type as the boto3/powertools library models):
handler `(module_path, class_name, function_name)` → the AWS SDK operations it
invokes. Built by tracing each entry-point symbol's call graph (gopls).

```jsonc
{
  "library_name": "terraform-provider-aws",
  "language": "go",
  "version": "v6.34.0",                      // surfaced in `--version --verbose`
  "call_patterns": [
    {
      "module_path": "s3",                   // service package (join key)
      "class_name": null,                    // set for methods, e.g. "servicePackage"
      "function_name": "resourceBucketRead",
      "call_type": "function",
      "sdk_operations": [
        { "service": "s3", "operation": "GetBucketAcl" }
        // ...
      ]
    },
    {
      "module_path": "s3", "class_name": "servicePackage", "function_name": "ListTags",
      "call_type": "instance_method",
      "sdk_operations": [ { "service": "s3", "operation": "GetBucketTagging" } ]
    }
  ]
}
```

- `sdk_operations[].service` is the **dashed botocore service id** (e.g.
  `chime-sdk-voice`), the same currency the source-code extractors emit; the
  enrichment layer maps it to the IAM prefix.
- The `ListTags`/`UpdateTags` patterns are stored **once per service** and
  referenced by every tagged resource via the CRUD map's `tags` block — no
  per-resource duplication.

## How they fit together (consumer)

For each resource change in a `terraform show -json` plan, the mapper
(`plan_to_calls/mapper.rs`):

1. looks up the resource's CRUD entry, selects slots from the plan `actions`
   (Create/Read/Update/Delete; **Read is always included**);
2. resolves each slot's handler symbol → `call_pattern` → SDK ops;
3. applies the **tag-call rule** (the provider interceptor's contract):
   - **Read** present ⇒ `ListTags` ops
   - **Create**/**Update** present ⇒ `UpdateTags` ops **and** `ListTags` ops (read-back)
   - **Delete** ⇒ no tag call
4. emits `SdkMethodCall`s into the existing enrichment + policy pipeline.

## Regeneration

Both files regenerate from the submodule via `xtask` (the weekly GitHub action
`.github/workflows/weekly_terraform_model_update.yml` does this and PRs the diff
only when an artifact or `names_data.hcl` changes). To run manually (needs Go +
gopls on PATH; the model build takes ~15 min):

```bash
# 1. resource → CRUD/tag symbols (fast)
cargo run -p xtask -- extract-terraform-crud-map \
  --terraform-provider-aws-root iam-policy-autopilot-policy-generation/resources/config/terraform/terraform-provider-aws \
  --output iam-policy-autopilot-policy-generation/resources/config/terraform/terraform-crud-map.json

# 2. symbols → SDK operations via call-graph extraction (slow, gopls)
cargo run -p xtask -- build-terraform-model \
  --crud-map iam-policy-autopilot-policy-generation/resources/config/terraform/terraform-crud-map.json \
  --terraform-provider-aws-root iam-policy-autopilot-policy-generation/resources/config/terraform/terraform-provider-aws \
  --output iam-policy-autopilot-policy-generation/resources/config/terraform/terraform-model.json --pretty
```

The Go extractor source lives at `xtask/extract-terraform-crud-map/go/main.go`
(copied into the submodule's `tools/` to run, since it imports provider
`internal/` packages). `NonSparseSubmodule` materializes the otherwise-sparse
submodule for the run and restores it afterward.

## Known gaps

- **resourceType switch-arm precision:** for the ~3 services whose `ListTags`
  switches on `resourceType` with *divergent* arms (S3, IAM), the model build
  currently extracts the whole method, so a tagged resource may get sibling
  resourceTypes' tag ops too (a superset — never causes AccessDenied, just
  slightly broad). The `tags.resource_type` field is recorded to enable precise
  arm-scoping later.
- See also the in-repo design doc `docs/design/terraform-plan-to-policy.md`.
