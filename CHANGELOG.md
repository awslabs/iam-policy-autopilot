## [Unreleased]

### Added
- Support for chained and nested boto3 sub-resource actions, including calls on a variable bound to a chain — e.g. `s3.Bucket("b").put_object(...)`, `s3.Bucket("b").Object("k").put(...)`, and `obj = s3.Bucket("b").Object("k"); obj.put(...)` now resolve to the underlying operation with identifiers injected from the chain

### Changed

- An unrecognized method on a known boto3 resource no longer expands to every action of that resource. Such calls now contribute no permissions instead of over-approximating
- Generated policy statements are now sorted globally by service before being assigned to policies, so statements for the same service stay together (within size limits), producing deterministic, review-friendly output and stable diffs. Note: when cross-service action merging is enabled (`--minimize-policy-size` / `allow_cross_service_merging = true`), statements are grouped by shared resource rather than by service, so a single service's actions may be merged into a statement keyed on another service and the "same service stays together" grouping does not hold. This is expected for that option, which exists to produce more compact policies; the sort remains deterministic. (#153)

### Fixed

- `cloudwatch:PutMetricData` no longer scopes to `dataset/*` resources — the AWS service reference added `dataset` as a resource type, but regular custom metric publishing requires `Resource: "*"`

## [0.2.3] - 2026-06-17

### Added

- Variable type tracking for boto3 clients and resources — improves extraction precision when clients are passed across function boundaries (#128)
- Support for `boto3.Session().client()` / `.resource()` patterns in variable type tracking (#232)
- `--resource-cutoff` CLI flag and `resource_cutoff` MCP input to configure when resource lists collapse to `*` (#217)
- Support for namespace imports in TypeScript/JavaScript (#190)
- Added partial support for permissions needed by [aws-lambda-powertools](https://pypi.org/project/aws-lambda-powertools/) (#186)

### Fixed

- We now respect the system's native certificate store instead of using bundled certificates (#209)
- `--explain` now shows every call site when the same operation appears multiple times (#188)
- Condition values for the same key are now merged instead of overwritten when serializing policies (#199)

### Changed

- `EnrichmentEngine::new` now requires a `resource_cutoff` parameter; use `DEFAULT_RESOURCE_CUTOFF` to preserve existing behavior (#217)

## [0.2.2] - 2026-06-05

### Fixed

- Made output of `generate-policies` deterministic
- `fix_access_denied` MCP tool now correctly generates the policy from the error message and applies it after confirmation
- `fix_access_denied` MCP tool now fails when the client does not support elicitation
- `fix_access_denied` MCP tool now correctly handles user decline/cancel during elicitation instead of returning an error
- Fixed elicitation schema for `fix_access_denied` to use a valid object schema (`{"confirmed": bool}`) instead of a bare boolean that was rejected by MCP clients

### Changed

- The `fix_access_denied` MCP tool no longer accepts an input policy, but a resource override instead. The tool derives the policy from the error message, and if a resource override is provided, uses it. It then surfaces the policy to the user and after confirmation applies it.

## [0.2.1] - 2026-05-08

### Fixed

- Updated MCP server dependencies (#194)

## [0.2.0] - 2026-05-05

### Added

- IAM Policy Autopilot now supports policy generation for Java applications. (#134)
- When provided with Terraform configurations or plans, IAM Policy Autopilot now generates more precise resource blocks, e.g., narrowing arn:aws:s3:::* down to the actual bucket/resource referenced. (#157)
- IAM Policy Autopilot now supports overriding the default HTTP bind address of the MCP server. (#159)
- This release adds anonymous usage telemetry. Set IAM_POLICY_AUTOPILOT_TELEMETRY=0 to disable. See TELEMETRY.md for details (#174)

### Fixed

- Added support for EU sovereign cloud partition. Providing `--region eusc-de-east-1` will generate policies for the EU sovereign cloud. (#103)
- Fixed issues where we did not correctly convert casing when analyzing Python applications (#163)

## [0.1.4] - 2026-01-30

### Added

- Added `--explain` feature with action pattern filtering to output the reasons for why actions were added to the policy. Supports wildcards (e.g., `--explain '*'` for all, `--explain 's3:*'` for S3 actions). The explanations allow to review the operations which static analysis extracted from source code, and to correct them using the `--service-hints` flag, if necessary. (#84, #122)
- Added Kiro Power config (#69)
- Added submodule version and data hash info to `--version --verbose` output (#87)

### Changed

- Updated botocore and boto3 submodules (#126)

## [0.1.3] - 2026-01-26

### Fixed

- Add type hints for fix_access_denied for strict schema checks (#117)

## [0.1.2] - 2025-12-15

### Fixed

- Use SDK info to find the operation from a method name. Fixes a bug where `modify_db_cluster` (and similar names) was renamed incorrectly to `ModifyDbCluster` instead of `ModifyDBCluster`. (#70)
- Reduce false positive findings by fixing Go SDK parameter extraction. It now uses required arguments correctly to disambiguate possible services. (#50)

### Added

- Added installation script for MacOS and Linux. (#44)

### Changed

- We now add the policy ID `IamPolicyAutopilot` in the access denied workflow.  (#48)
- Updated Cargo.toml description. (#46)

## [0.1.1] - 2025-11-26

### Added

- Initial release
