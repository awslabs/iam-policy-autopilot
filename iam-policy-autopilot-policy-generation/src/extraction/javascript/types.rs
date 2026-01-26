//! JavaScript/TypeScript specific data types for AWS SDK extraction

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::Location;

/// Information about a single import with rename support
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ImportInfo {
    /// Original name in the AWS SDK (e.g., "S3Client", "PutObjectCommand")
    pub(crate) original_name: String,
    /// Import statement
    pub(crate) statement: String,
    /// Local name used in the code (e.g., "MyS3Client", "PutObject")
    pub(crate) local_name: String,
    /// Whether this import was renamed (original_name != local_name)
    pub(crate) is_renamed: bool,
    /// Location of the import
    pub(crate) location: Location,
}

impl ImportInfo {
    /// Create a new ImportInfo with the given names and line position
    pub(crate) fn new(
        original_name: String,
        local_name: String,
        statement: &str,
        location: Location,
    ) -> Self {
        let is_renamed = original_name != local_name;
        Self {
            original_name,
            statement: statement.to_string(),
            local_name,
            is_renamed,
            location,
        }
    }
}

/// Information about imports from a specific AWS SDK sublibrary
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SublibraryInfo {
    /// AWS SDK sublibrary name (e.g., "client-s3", "lib-dynamodb")
    pub(crate) sublibrary: String,
    /// List of imported items
    pub(crate) imports: Vec<ImportInfo>,
    /// Mapping from local names to original names for quick lookup
    pub(crate) name_mappings: HashMap<String, String>,
}

impl SublibraryInfo {
    /// Create a new SublibraryInfo
    pub(crate) fn new(sublibrary: String) -> Self {
        Self {
            sublibrary,
            imports: Vec::new(),
            name_mappings: HashMap::new(),
        }
    }

    /// Add an import to this sublibrary
    pub(crate) fn add_import(&mut self, import_info: ImportInfo) {
        self.name_mappings.insert(
            import_info.local_name.clone(),
            import_info.original_name.clone(),
        );
        self.imports.push(import_info);
    }
}

/// Information about a client instantiation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClientInstantiation {
    /// Variable name (e.g., "s3Client")
    pub(crate) variable: String,
    /// Local client type name (might be renamed)
    pub(crate) client_type: String,
    /// Original AWS client type name
    pub(crate) original_client_type: String,
    /// AWS SDK sublibrary (e.g., "client-s3")
    pub(crate) sublibrary: String,
    /// Arguments passed to constructor
    pub(crate) arguments: HashMap<String, String>,
    /// Line number where instantiation occurs
    pub(crate) line: usize,
}

/// Information about valid AWS client types discovered from imports
#[derive(Debug, Clone)]
pub(crate) struct ValidClientTypes {
    /// List of valid local client type names (e.g., "S3Client", "MyS3Client")
    pub(crate) client_types: Vec<String>,
    /// Mapping from local names to original AWS names (handles renames)
    pub(crate) name_mappings: HashMap<String, String>,
    /// Mapping from local names to their AWS SDK sublibraries
    pub(crate) sublibrary_mappings: HashMap<String, String>,
}

impl ValidClientTypes {
    /// Create a new ValidClientTypes instance
    pub(crate) fn new(
        client_types: Vec<String>,
        name_mappings: HashMap<String, String>,
        sublibrary_mappings: HashMap<String, String>,
    ) -> Self {
        Self {
            client_types,
            name_mappings,
            sublibrary_mappings,
        }
    }

    /// Check if this collection is empty
    pub(crate) fn is_empty(&self) -> bool {
        self.client_types.is_empty()
    }
}

/// Information about a method call (non-send)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MethodCall {
    /// Client variable name
    pub(crate) client_variable: String,
    /// Local client type name
    pub(crate) client_type: String,
    /// Original AWS client type name
    pub(crate) original_client_type: String,
    /// Client sublibrary
    pub(crate) client_sublibrary: String,
    /// Method name
    pub(crate) method_name: String,
    /// Method arguments
    pub(crate) arguments: HashMap<String, String>,
    /// Matched expression
    pub(crate) expr: String,
    /// Location where the method call was found
    pub(crate) location: Location,
}

/// Combined results from all scanning operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct JavaScriptScanResults {
    /// Import information
    pub(crate) imports: Vec<SublibraryInfo>,
    /// Require information  
    pub(crate) requires: Vec<SublibraryInfo>,
    /// Client instantiations
    pub(crate) client_instantiations: Vec<ClientInstantiation>,
    /// Method calls
    pub(crate) method_calls: Vec<MethodCall>,
}

impl JavaScriptScanResults {
    /// Create new empty scan results
    pub(crate) fn new() -> Self {
        Self {
            imports: Vec::new(),
            requires: Vec::new(),
            client_instantiations: Vec::new(),
            method_calls: Vec::new(),
        }
    }
}

impl Default for JavaScriptScanResults {
    fn default() -> Self {
        Self::new()
    }
}
