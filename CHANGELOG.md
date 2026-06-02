## [Unreleased]

### Added

- Support for namespace imports in TypeScript/JavaScript (#190)
- `--resource-cutoff` CLI flag and `resource_cutoff` MCP input to configure when resource lists collapse to `*` (#217)

### Fixed

- `fix_access_denied` MCP tool now applies the user-confirmed policy instead of regenerating one (#202)
- `--explain` now shows every call site when the same operation appears multiple times (#188)
- Condition values for the same key are now merged instead of overwritten when serializing policies (#199)

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
