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
