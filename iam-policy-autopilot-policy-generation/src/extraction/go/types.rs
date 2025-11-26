//! Go-specific data types for AWS SDK extraction

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Information about a single import with rename support for Go
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ImportInfo {
    /// Original import path (e.g., "github.com/aws/aws-sdk-go-v2/service/s3")
    pub(crate) original_name: String,
    /// Local name used in the code (e.g., "s3", "myS3")
    pub(crate) local_name: String,
    /// Whether this import was renamed (original_name != local_name)
    pub(crate) is_renamed: bool,
    /// Line number where this import appears
    pub(crate) line: usize,
    /// Extracted service name from the import path (e.g., "s3" from "github.com/aws/aws-sdk-go-v2/service/s3")
    pub(crate) service_name: Option<String>,
}

impl ImportInfo {
    /// Create a new ImportInfo with the given names and line position
    pub(crate) fn new(original_name: String, local_name: String, line: usize) -> Self {
        let is_renamed = original_name != local_name;
        let service_name = Self::extract_service_name(&original_name);

        Self {
            original_name,
            local_name,
            is_renamed,
            line,
            service_name,
        }
    }

    /// Extract service name from AWS SDK import path
    /// Examples:
    /// - "github.com/aws/aws-sdk-go-v2/service/s3" -> Some("s3")
    /// - "github.com/aws/aws-sdk-go-v2/service/dynamodb" -> Some("dynamodb")
    /// - "github.com/aws/aws-sdk-go-v2/aws" -> None (not a service)
    fn extract_service_name(import_path: &str) -> Option<String> {
        // Check if this is an AWS SDK service import
        if import_path.starts_with("github.com/aws/aws-sdk-go-v2/service/") {
            // Extract the service name after the last slash
            if let Some(service) = import_path.strip_prefix("github.com/aws/aws-sdk-go-v2/service/")
            {
                // Handle cases where there might be additional path components
                let service_name = service.split('/').next().unwrap_or(service);
                return Some(service_name.to_string());
            }
        }
        None
    }
}

/// Collection of import information for Go files
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GoImportInfo {
    /// List of all imports found in the file
    pub(crate) imports: Vec<ImportInfo>,
    /// Mapping from local names to service names for quick lookup
    pub(crate) service_mappings: HashMap<String, String>,
}

impl GoImportInfo {
    /// Create a new empty GoImportInfo
    pub(crate) fn new() -> Self {
        Self {
            imports: Vec::new(),
            service_mappings: HashMap::new(),
        }
    }

    /// Add an import to this collection
    pub(crate) fn add_import(&mut self, import_info: ImportInfo) {
        // If this import has a service name, add it to the mappings
        if let Some(ref service_name) = import_info.service_name {
            self.service_mappings
                .insert(import_info.local_name.clone(), service_name.clone());
        }
        self.imports.push(import_info);
    }

    /// Get all AWS service names that are imported
    pub(crate) fn get_imported_services(&self) -> Vec<String> {
        self.service_mappings.values().cloned().collect()
    }
}

impl Default for GoImportInfo {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_service_name() {
        // Test valid AWS SDK service imports
        assert_eq!(
            ImportInfo::extract_service_name("github.com/aws/aws-sdk-go-v2/service/s3"),
            Some("s3".to_string())
        );
        assert_eq!(
            ImportInfo::extract_service_name("github.com/aws/aws-sdk-go-v2/service/dynamodb"),
            Some("dynamodb".to_string())
        );
        assert_eq!(
            ImportInfo::extract_service_name("github.com/aws/aws-sdk-go-v2/service/ec2"),
            Some("ec2".to_string())
        );

        // Test non-service AWS SDK imports
        assert_eq!(
            ImportInfo::extract_service_name("github.com/aws/aws-sdk-go-v2/aws"),
            None
        );
        assert_eq!(
            ImportInfo::extract_service_name("github.com/aws/aws-sdk-go-v2/config"),
            None
        );

        // Test non-AWS imports
        assert_eq!(ImportInfo::extract_service_name("fmt"), None);
        assert_eq!(ImportInfo::extract_service_name("context"), None);
        assert_eq!(
            ImportInfo::extract_service_name("github.com/some/other/package"),
            None
        );
    }

    #[test]
    fn test_import_info_creation() {
        let import_info = ImportInfo::new(
            "github.com/aws/aws-sdk-go-v2/service/s3".to_string(),
            "s3".to_string(),
            10,
        );

        assert_eq!(
            import_info.original_name,
            "github.com/aws/aws-sdk-go-v2/service/s3"
        );
        assert_eq!(import_info.local_name, "s3");
        assert!(import_info.is_renamed); // This is renamed since original != local
        assert_eq!(import_info.line, 10);
        assert_eq!(import_info.service_name, Some("s3".to_string()));
    }

    #[test]
    fn test_import_info_with_rename() {
        let import_info = ImportInfo::new(
            "github.com/aws/aws-sdk-go-v2/service/s3".to_string(),
            "myS3".to_string(),
            15,
        );

        assert_eq!(
            import_info.original_name,
            "github.com/aws/aws-sdk-go-v2/service/s3"
        );
        assert_eq!(import_info.local_name, "myS3");
        assert!(import_info.is_renamed);
        assert_eq!(import_info.line, 15);
        assert_eq!(import_info.service_name, Some("s3".to_string()));
    }

    #[test]
    fn test_go_import_info_operations() {
        let mut go_imports = GoImportInfo::new();

        // Add some imports
        go_imports.add_import(ImportInfo::new(
            "github.com/aws/aws-sdk-go-v2/service/s3".to_string(),
            "s3".to_string(),
            5,
        ));
        go_imports.add_import(ImportInfo::new(
            "github.com/aws/aws-sdk-go-v2/service/dynamodb".to_string(),
            "ddb".to_string(),
            6,
        ));
        go_imports.add_import(ImportInfo::new("fmt".to_string(), "fmt".to_string(), 7));

        // Test getting imported services
        let services = go_imports.get_imported_services();
        assert_eq!(services.len(), 2);
        assert!(services.contains(&"s3".to_string()));
        assert!(services.contains(&"dynamodb".to_string()));
    }
}
