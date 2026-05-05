# Changelog

All notable changes to this project will be documented in this file.

## [0.2.0] - 2026-05-04

### Features

- IAM policy generation for Java applications — Customers using the AWS SDK for Java (v1 or v2) can now automatically extract the AWS API calls their code makes and generate least-privilege IAM policies from them — the same capability previously available only for Python/boto3 users. This eliminates manual policy authoring for Java workloads and reduces the risk of over-permissioned roles (#134)
- More precise IAM policies from Terraform — When generating or analyzing IAM policies for Terraform configurations, the tool now refines resource ARNs in policy blocks to be more specific (e.g., narrowing arn:aws:s3:::* down to the actual bucket/resource referenced). This helps customers achieve least-privilege policies directly from their Terraform code, reducing overly permissive access without manual ARN editing (#157)
- This release adds anonymous usage telemetry. Set IAM_POLICY_AUTOPILOT_TELEMETRY=0 to disable. See TELEMETRY.md for details (#174)
- Configurable HTTP bind address — Users can now override the default bind address (e.g., via a BIND_ADDRESS environment variable), making it straightforward to run the tool in containers, Kubernetes pods, or other environments (#159)

### Fixed

- Added support for EU sovereign cloud partition. Providing `--region eusc-de-east-1` will generate policies for the EU sovereign cloud.  (#103)
- Adopt botocore's snake_case conversion logic for AWS operation and waiter names, run it at build time (via Python), and embed the resulting lookup map into the Rust binary for both forward (PascalCase → snake_case) and reverse (snake_case → PascalCase) runtime lookups. Only non-trivial mappings are included to keep the binary small. (#163)

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

## Fixed

- Use SDK info to find the operation from a method name. Fixes a bug where `modify_db_cluster` (and similar names) was renamed incorrectly to `ModifyDbCluster` instead of `ModifyDBCluster`. (#70)
- Reduce false positive findings by fixing Go SDK parameter extraction. It now uses required arguments correctly to disambiguate possible services. (#50)

## Added

- Added installation script for MacOS and Linux. (#44)

## Changed

- We now add the policy ID `IamPolicyAutopilot` in the access denied workflow.  (#48)
- Updated Cargo.toml description. (#46)

## [0.1.1] - 2025-11-26

### 🚀 Features

- Initial release
