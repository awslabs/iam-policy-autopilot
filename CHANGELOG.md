# Changelog

All notable changes to this project will be documented in this file.

## [0.2.0] - 2026-05-04

### Added
- This release adds anonymous usage telemetry. Set IAM_POLICY_AUTOPILOT_TELEMETRY=0 to disable. See TELEMETRY.md for details (#174)
- Automated submodule update (#137)
- Allowing overriding http bind address (#159)
- Resource arn refinement using terraform (#157)
- Add support for Java language (#134)
- FAS model for redshift-data:ExecuteStatement (#181)

### Changed

- Replace duplicated `CallInfo` with shared `WaiterCallPattern` (#139)
- Replace duplicated paginator methods with shared `PaginatorCallPattern` (#149)

### Documentation
- Add telemetry documentation (#184)

### Fixed

- Support eu sovereign cloud (#103)
- Aws_python_case_conversion (#163)

## [Unreleased]

### Added

- Added support for EU sovereign cloud partition. Providing `--region eusc-de-east-1` will generate policies for the EU sovereign cloud. (#103)

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
