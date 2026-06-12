use std::collections::{HashMap, HashSet};

/// Type information for a boto3 variable
///
/// Stores both the AWS service name (for current logic) and optional
/// full type information from LSP (for future precision improvements).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct VariableTypeInfo {
    /// AWS service name (e.g., "s3", "dynamodb")
    /// Used for current disambiguation and enrichment logic
    pub(crate) service_name: String,

    /// Full qualified type from LSP (optional)
    /// e.g., "mypy_boto3_s3.client.S3Client"
    /// Preserved for future precision improvements
    pub(crate) qualified_type: Option<String>,

    /// SDK object kind (optional)
    /// Helps distinguish clients, resources, paginators, waiters
    pub(crate) kind: Option<SdkObjectKind>,
}

impl VariableTypeInfo {
    /// Create from service name with inferred kind (pattern matching)
    ///
    /// Used when we can infer the kind from the pattern we matched.
    /// For example: `boto3.client('s3')` → Client, `boto3.resource('s3')` → Resource
    pub(crate) fn from_service_with_kind(service_name: String, kind: SdkObjectKind) -> Self {
        Self {
            service_name,
            qualified_type: None,
            kind: Some(kind),
        }
    }

    /// Create from LSP type information (future use)
    ///
    /// This will be used when we integrate with a real LSP server to get
    /// full qualified types like "mypy_boto3_s3.client.S3Client".
    #[allow(dead_code)]
    pub(crate) fn from_lsp_type(
        qualified_type: String,
        service_name: String,
        kind: SdkObjectKind,
    ) -> Self {
        Self {
            service_name,
            qualified_type: Some(qualified_type),
            kind: Some(kind),
        }
    }
}

/// Kind of SDK object
///
/// Distinguishes between different boto3 object types to enable
/// more precise operation validation in the future.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum SdkObjectKind {
    /// boto3.client('service') - Low-level client
    Client,
    /// boto3.resource('service') - High-level resource
    Resource,
    /// resource.Table('name'), s3.Bucket('name'), etc.
    ResourceCollection,
}

/// Tracks boto3 client and resource variable assignments
///
/// Maps variable names to their type information including AWS service,
/// full qualified type (from LSP), and object kind.
#[derive(Debug, Default)]
pub(crate) struct VariableTypeTracker {
    /// Module-level variable assignments: variable_name -> type_info
    pub(super) module_scope: HashMap<String, VariableTypeInfo>,

    /// Function-level variable assignments: function_name -> (variable_name -> type_info)
    pub(super) function_scopes: HashMap<String, HashMap<String, VariableTypeInfo>>,

    /// Parameter mappings: (function_name, param_name) -> set of possible type_info
    pub(super) parameter_types: HashMap<(String, String), HashSet<VariableTypeInfo>>,

    /// Function names that appear multiple times (e.g., methods in different classes).
    /// Lookups against these names return None to avoid false narrowing.
    /// TODO: Resolve properly by qualifying with class name (e.g., "ClassName.method").
    pub(super) conflicted_functions: HashSet<String>,
}

impl VariableTypeTracker {
    pub(crate) fn new() -> Self {
        Self {
            module_scope: HashMap::new(),
            function_scopes: HashMap::new(),
            parameter_types: HashMap::new(),
            conflicted_functions: HashSet::new(),
        }
    }
}
