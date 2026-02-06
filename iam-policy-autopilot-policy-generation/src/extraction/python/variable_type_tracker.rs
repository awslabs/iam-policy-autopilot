//! Variable type tracking for boto3 clients and resources
//!
//! This module tracks boto3 client and resource assignments to improve
//! SDK method call extraction precision when variables are passed across
//! function boundaries.
//!
//! # What This Module Tracks
//!
//! ## Direct boto3 Assignments
//! - **Clients**: `s3_client = boto3.client('s3')` (module and function level)
//! - **Resources**: `dynamodb = boto3.resource('dynamodb')` (module and function level)
//!
//! ## Variable Aliases
//! - **Simple aliases**: `my_client = s3_client` (module and function level)
//! - **Chained aliases**: `client_a = s3_client; client_b = client_a`
//!
//! ## Function Parameter Type Inference
//! - Tracks parameter types from call sites: `upload_file(s3_client)` → `client` parameter is `s3`
//! - Supports multiple types per parameter: `process(s3_client)` and `process(ec2_client)` → `client` can be `{s3, ec2}`
//!
//! ## Resource-Derived Variables
//! - **DynamoDB tables**: `table = dynamodb.Table('users')` → `table` is `dynamodb` service
//! - **S3 buckets**: `bucket = s3.Bucket('my-bucket')` → `bucket` is `s3` service
//! - **S3 objects**: `obj = bucket.Object('key')` → `obj` is `s3` service (inherits from bucket)
//! - Any resource method call: `derived = resource.SomeMethod(args)` → inherits service from `resource`
//!
//! ## Python Scoping (LEGB)
//! Follows Python's LEGB scoping rules:
//! - **Local**: Function-level variables and parameters shadow module-level
//! - **Global**: Module-level variables
//!
//! # What This Module Does NOT Track
//!
//! ## Paginators and Waiters (Handled Separately)
//! - **Paginators**: `paginator = client.get_paginator('operation')` - Handled by `paginator_extractor.rs`
//! - **Waiters**: `waiter = client.get_waiter('waiter_name')` - Handled by `waiters_extractor.rs`
//!
//! These have specialized extraction logic and currently don't need cross-function variable tracking.
//! The extractors match direct usage patterns like `client.get_paginator('op').paginate()`.
//!
//! # Future Enhancements
//!
//! ## Cross-function tracking for paginators/waiters
//! - Track paginator variables: `paginator = client.get_paginator('op')` then later `paginator.paginate()`
//! - Track waiter variables: `waiter = client.get_waiter('name')` then later `waiter.wait()`
//!
//! ## Advanced tracking
//! - **Function return values**: `def create_client(): return boto3.client('s3')`
//! - **Class attributes**: `self.client = boto3.client('s3')`
//! - **Conditional assignments**: Type narrowing based on control flow
//!
//! ## Session support
//! - `session.client('service')` - Track session-based client creation
//! - `session.resource('service')` - Track session-based resource creation

use crate::extraction::python::node_kinds;
use crate::extraction::AstWithSourceFile;
use ast_grep_language::Python;
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
#[allow(dead_code)] // Paginator and Waiter variants are for future work
pub(crate) enum SdkObjectKind {
    /// boto3.client('service') - Low-level client
    Client,
    /// boto3.resource('service') - High-level resource
    Resource,
    /// client.get_paginator('operation') - Paginator
    /// TODO: Track paginator variables to enable cross-function paginator usage.
    /// Currently handled by paginator_extractor.rs which only matches direct patterns.
    /// Example: `paginator = client.get_paginator('list_objects_v2')` then `paginator.paginate()`
    Paginator,
    /// client.get_waiter('waiter') - Waiter
    /// TODO: Track waiter variables to enable cross-function waiter usage.
    /// Currently handled by waiters_extractor.rs which only matches direct patterns.
    /// Example: `waiter = client.get_waiter('instance_running')` then `waiter.wait()`
    Waiter,
    /// resource.Table('name'), s3.Bucket('name'), etc.
    ResourceCollection,
}

/// Tracks boto3 client and resource variable assignments
///
/// Maps variable names to their type information including AWS service,
/// full qualified type (from LSP), and object kind.
///
/// # Implementation Details
///
/// ## Tracked Patterns
/// 1. **Direct assignments**: `s3_client = boto3.client('s3')`
/// 2. **Aliases**: `my_client = s3_client`
/// 3. **Function parameters**: Inferred from call sites
/// 4. **Resource-derived variables**: `table = dynamodb.Table('users')`
///
/// ## Scoping
/// - **Module scope**: Variables assigned at module level
/// - **Function scope**: Variables assigned within function bodies
/// - **Parameters**: Function parameters with types inferred from call sites
///
/// ## Multiple Service Types for Parameters
/// Since Python allows the same function to be called with different argument types,
/// we track ALL possible service types for each parameter:
/// ```python
/// def process(client):
///     client.put_object(...)  # Could be S3 or other service
///
/// s3 = boto3.client('s3')
/// ec2 = boto3.client('ec2')
/// process(s3)   # client → s3
/// process(ec2)  # client → ec2
/// # Result: client parameter has possible services {s3, ec2}
/// ```
///
/// ## Python LEGB Scoping
/// Lookups follow Python's LEGB (Local, Enclosing, Global, Built-in) rules:
/// - Function-local variables and parameters are checked first
/// - Module-level variables are checked second
/// - Parameters can shadow module-level variables
#[derive(Debug, Default)]
pub(crate) struct VariableTypeTracker {
    /// Module-level variable assignments: variable_name -> type_info
    module_scope: HashMap<String, VariableTypeInfo>,

    /// Function-level variable assignments: function_name -> (variable_name -> type_info)
    /// Tracks variables assigned within function bodies (including aliases)
    function_scopes: HashMap<String, HashMap<String, VariableTypeInfo>>,

    /// Parameter mappings: (function_name, param_name) -> set of possible type_info
    /// Tracks ALL possible service types a parameter might have across different call sites
    parameter_types: HashMap<(String, String), HashSet<VariableTypeInfo>>,
}

impl VariableTypeTracker {
    /// Create a new VariableTypeTracker
    pub(crate) fn new() -> Self {
        Self {
            module_scope: HashMap::new(),
            function_scopes: HashMap::new(),
            parameter_types: HashMap::new(),
        }
    }

    /// Track boto3.client() and boto3.resource() assignments in the AST
    ///
    /// This is the main entry point that orchestrates all tracking patterns:
    ///
    /// 1. **Client assignments**: `boto3.client('service')` at module and function level
    /// 2. **Resource assignments**: `boto3.resource('service')` at module and function level
    /// 3. **Aliases**: `my_client = s3_client` at module and function level
    /// 4. **Function calls**: Infer parameter types from arguments at call sites
    /// 5. **Resource-derived variables**: `table = dynamodb.Table('name')`, `bucket = s3.Bucket('name')`
    ///
    /// Note: Paginators and waiters are explicitly skipped and handled by separate extractors.
    pub(crate) fn track_boto3_assignments(&mut self, ast: &AstWithSourceFile<Python>) {
        let root = ast.ast.root();

        // Pattern 1: boto3.client('service_name') - module and function level
        self.track_client_assignments(&root);

        // Pattern 2: boto3.resource('service_name') - module and function level
        self.track_resource_assignments(&root);

        // Pattern 3: Simple aliases (var = other_var) within functions
        self.track_aliases(&root);

        // Pattern 4: Function calls with known client arguments
        self.track_function_calls(&root);

        // Pattern 5: Resource-derived variables (Table, Bucket, etc.)
        self.track_resource_derived_variables(&root);
    }

    /// Track boto3.client() assignments
    fn track_client_assignments(
        &mut self,
        root: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) {
        // First, track function-level assignments
        // Pattern matches function definitions with any parameters (including none)
        let func_def_pattern = "def $FUNC($$$): $$$BODY";
        for func_match in root.find_all(func_def_pattern) {
            log::debug!("FUNC_MATCH: {}", func_match.text());
            let env = func_match.get_env();

            let func_name = if let Some(node) = env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };

            // Find boto3.client() assignments within this function
            let pattern = "$VAR = boto3.client($SERVICE)";
            for node_match in func_match.get_node().find_all(pattern) {
                log::debug!("NODE_MATCH: {}", node_match.text());
                let assign_env = node_match.get_env();

                let var_name = if let Some(var_node) = assign_env.get_match("VAR") {
                    var_node.text().to_string()
                } else {
                    continue;
                };

                let service_name = if let Some(service_node) = assign_env.get_match("SERVICE") {
                    let raw_text = service_node.text().to_string();
                    self.extract_string_literal(&raw_text)
                } else {
                    continue;
                };

                log::debug!(
                    "Tracked boto3.client assignment in function '{}': {} -> {}",
                    func_name,
                    var_name,
                    service_name
                );

                // Add to function scope
                self.function_scopes
                    .entry(func_name.clone())
                    .or_default()
                    .insert(
                        var_name,
                        VariableTypeInfo::from_service_with_kind(
                            service_name,
                            SdkObjectKind::Client,
                        ),
                    );
            }
        }

        // Then track module-level assignments
        let pattern = "$VAR = boto3.client($SERVICE)";
        for node_match in root.find_all(pattern) {
            log::debug!("NODE_MATCH (MODULE): {}", node_match.text());
            let env = node_match.get_env();

            // Check if this assignment is inside a function definition
            // Walk up the tree to see if we're inside a function_definition node
            let mut current = node_match.get_node().parent();
            let mut is_inside_function = false;
            while let Some(node) = current {
                if node.kind() == node_kinds::FUNCTION_DEFINITION {
                    is_inside_function = true;
                    break;
                }
                current = node.parent();
            }

            if is_inside_function {
                continue; // Skip function-level assignments (already handled above)
            }

            let var_name = if let Some(var_node) = env.get_match("VAR") {
                var_node.text().to_string()
            } else {
                continue;
            };

            let service_name = if let Some(service_node) = env.get_match("SERVICE") {
                let raw_text = service_node.text().to_string();
                self.extract_string_literal(&raw_text)
            } else {
                continue;
            };

            log::debug!(
                "Tracked boto3.client assignment at module level: {} -> {}",
                var_name,
                service_name
            );
            self.module_scope.insert(
                var_name,
                VariableTypeInfo::from_service_with_kind(service_name, SdkObjectKind::Client),
            );
        }
    }

    /// Track boto3.resource() assignments
    fn track_resource_assignments(
        &mut self,
        root: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) {
        // First, track function-level assignments
        let func_def_pattern = "def $FUNC($$$PARAMS):$$$BODY";
        for func_match in root.find_all(func_def_pattern) {
            let env = func_match.get_env();

            let func_name = if let Some(node) = env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };
            // Find boto3.resource() assignments within this function
            let pattern = "$VAR = boto3.resource($SERVICE)";
            for node_match in func_match.get_node().find_all(pattern) {
                let assign_env = node_match.get_env();

                let var_name = if let Some(var_node) = assign_env.get_match("VAR") {
                    var_node.text().to_string()
                } else {
                    continue;
                };

                let service_name = if let Some(service_node) = assign_env.get_match("SERVICE") {
                    let raw_text = service_node.text().to_string();
                    self.extract_string_literal(&raw_text)
                } else {
                    continue;
                };

                log::debug!(
                    "Tracked boto3.resource assignment in function '{}': {} -> {}",
                    func_name,
                    var_name,
                    service_name
                );

                // Add to function scope
                self.function_scopes
                    .entry(func_name.clone())
                    .or_default()
                    .insert(
                        var_name,
                        VariableTypeInfo::from_service_with_kind(
                            service_name,
                            SdkObjectKind::Resource,
                        ),
                    );
            }
        }

        // Then track module-level assignments
        let pattern = "$VAR = boto3.resource($SERVICE)";
        for node_match in root.find_all(pattern) {
            let env = node_match.get_env();

            // Check if this assignment is inside a function definition
            // Walk up the tree to see if we're inside a function_definition node
            let mut current = node_match.get_node().parent();
            let mut is_inside_function = false;
            while let Some(node) = current {
                if node.kind() == node_kinds::FUNCTION_DEFINITION {
                    is_inside_function = true;
                    break;
                }
                current = node.parent();
            }

            if is_inside_function {
                continue; // Skip function-level assignments (already handled above)
            }

            let var_name = if let Some(var_node) = env.get_match("VAR") {
                var_node.text().to_string()
            } else {
                continue;
            };

            let service_name = if let Some(service_node) = env.get_match("SERVICE") {
                let raw_text = service_node.text().to_string();
                self.extract_string_literal(&raw_text)
            } else {
                continue;
            };

            log::debug!(
                "Tracked boto3.resource assignment at module level: {} -> {}",
                var_name,
                service_name
            );
            self.module_scope.insert(
                var_name,
                VariableTypeInfo::from_service_with_kind(service_name, SdkObjectKind::Resource),
            );
        }
    }

    /// Track simple variable aliases within functions
    /// Pattern: `my_client = s3_client` where s3_client is already tracked
    fn track_aliases(
        &mut self,
        root: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) {
        // First, find all function definitions to know which assignments are in which function
        let func_def_pattern = "def $FUNC($$$PARAMS):$$$BODY";

        for func_match in root.find_all(func_def_pattern) {
            let env = func_match.get_env();

            let func_name = if let Some(node) = env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };

            let body_node = if let Some(node) = env.get_match("BODY") {
                node
            } else {
                continue;
            };

            // Find all assignments within this function body
            let pattern = "$NEW = $OLD";
            for node_match in body_node.find_all(pattern) {
                let assign_env = node_match.get_env();

                let new_var = if let Some(node) = assign_env.get_match("NEW") {
                    node.text().to_string()
                } else {
                    continue;
                };

                let old_var = if let Some(node) = assign_env.get_match("OLD") {
                    node.text().to_string()
                } else {
                    continue;
                };

                // Check if old_var is a tracked variable (in module scope or function scope)
                // Clone the type info to avoid borrow checker issues
                let type_info = self
                    .get_type_info_for_variable_in_context(&old_var, Some(&func_name))
                    .cloned();

                if let Some(type_info) = type_info {
                    log::debug!(
                        "Tracked alias in function '{}': {} -> {} (service: {})",
                        func_name,
                        new_var,
                        old_var,
                        type_info.service_name
                    );

                    // Add to function scope
                    self.function_scopes
                        .entry(func_name.clone())
                        .or_default()
                        .insert(new_var, type_info);
                }
            }
        }

        // Also track module-level aliases (assignments outside functions)
        let pattern = "$NEW = $OLD";
        for node_match in root.find_all(pattern) {
            let env = node_match.get_env();

            // Check if this assignment is inside a function definition
            let mut current = node_match.get_node().parent();
            let mut is_inside_function = false;
            while let Some(node) = current {
                if node.kind() == node_kinds::FUNCTION_DEFINITION {
                    is_inside_function = true;
                    break;
                }
                current = node.parent();
            }

            if is_inside_function {
                continue; // Skip function-level assignments (already handled above)
            }

            let new_var = if let Some(node) = env.get_match("NEW") {
                node.text().to_string()
            } else {
                continue;
            };

            let old_var = if let Some(node) = env.get_match("OLD") {
                node.text().to_string()
            } else {
                continue;
            };

            // Check if old_var is a tracked variable
            if let Some(type_info) = self.module_scope.get(&old_var) {
                log::debug!(
                    "Tracked module-level alias: {} -> {} (service: {})",
                    new_var,
                    old_var,
                    type_info.service_name
                );
                self.module_scope.insert(new_var, type_info.clone());
            }
        }
    }

    /// Track function calls to infer parameter types
    /// Pattern: `upload_data(s3_client, dynamodb_client, ...)` where clients are tracked
    fn track_function_calls(
        &mut self,
        root: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) {
        // First, build a map of function names to their parameter lists
        let mut func_params: HashMap<String, Vec<String>> = HashMap::new();

        // Find all function definitions: def func_name(args):
        let func_def_pattern = "def $FUNC($$$PARAMS):$$$BODY";
        for node_match in root.find_all(func_def_pattern) {
            let env = node_match.get_env();

            let func_name = if let Some(func_node) = env.get_match("FUNC") {
                func_node.text().to_string()
            } else {
                continue;
            };

            let param_nodes = env.get_multiple_matches("PARAMS");
            let mut params = Vec::new();

            for param_node in param_nodes {
                let param_text = param_node.text().to_string().trim().to_string();
                // Skip commas and empty strings
                if param_text == "," || param_text.is_empty() {
                    continue;
                }
                // Remove default values and type annotations
                if let Some(param_name) = Self::extract_all_params(&param_text).into_iter().next() {
                    params.push(param_name);
                }
            }

            if !params.is_empty() {
                log::debug!("Found function definition: {}({:?})", func_name, params);
                func_params.insert(func_name, params);
            }
        }

        log::debug!("Function parameters map: {:?}", func_params);

        // Now find function calls and match arguments to parameters
        let call_pattern = "$FUNC($$$ARGS)";

        for node_match in root.find_all(call_pattern) {
            let env = node_match.get_env();

            let func_name = if let Some(node) = env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };

            let arg_nodes = env.get_multiple_matches("ARGS");
            let args: Vec<String> = arg_nodes
                .iter()
                .map(|node| node.text().to_string().trim().to_string())
                .filter(|arg| arg != "," && !arg.is_empty()) // Filter out commas and empty strings
                .collect();

            if args.is_empty() {
                continue;
            }

            log::debug!("Found function call: {}({:?})", func_name, args);

            // Check if we know the parameter names for this function
            if let Some(param_names) = func_params.get(&func_name) {
                // Match arguments to parameters positionally
                for (i, arg) in args.iter().enumerate() {
                    // Get the corresponding parameter name (if it exists)
                    if let Some(param_name) = param_names.get(i) {
                        // Check if the argument is a tracked variable (client or resource)
                        // Clone the type info before the mutable borrow to avoid borrow checker issues
                        let type_info = self
                            .get_type_info_for_variable_in_context(arg, None)
                            .cloned();

                        if let Some(type_info) = type_info {
                            log::debug!(
                                "Tracked function call: {}({}) - param '{}' (position {}) -> service '{}'",
                                func_name,
                                arg,
                                param_name,
                                i,
                                type_info.service_name
                            );
                            // Add type info to the set of possible types for this parameter
                            self.parameter_types
                                .entry((func_name.clone(), param_name.clone()))
                                .or_default()
                                .insert(type_info);
                        }
                    }
                }
            } else {
                log::debug!("No parameter mapping found for function {}", func_name);
            }
        }
    }

    /// Extract all parameters/arguments from a comma-separated list
    /// Examples:
    /// - "client" -> vec!["client"]
    /// - "client, bucket, key" -> vec!["client", "bucket", "key"]
    /// - "self, client, table" -> vec!["self", "client", "table"]
    /// - "" -> vec![]
    ///
    /// Handles:
    /// - Default values: "client=None" -> "client"
    /// - Type annotations: "client: str" -> "client"
    /// - Whitespace: " client , bucket " -> vec!["client", "bucket"]
    fn extract_all_params(params: &str) -> Vec<String> {
        let trimmed = params.trim();
        if trimmed.is_empty() {
            return vec![];
        }

        trimmed
            .split(',')
            .filter_map(|param| {
                let param = param.trim();
                if param.is_empty() {
                    return None;
                }

                // Remove default value assignment (e.g., "client=None" -> "client")
                let param = param.split('=').next()?.trim();

                // Remove type annotation (e.g., "client: str" -> "client")
                let param = param.split(':').next()?.trim();

                if param.is_empty() {
                    None
                } else {
                    Some(param.to_string())
                }
            })
            .collect()
    }

    /// Track resource-derived variables like Table, Bucket, etc.
    ///
    /// Patterns:
    /// - `table = dynamodb.Table('name')` -> table is dynamodb
    /// - `bucket = s3.Bucket('name')` -> bucket is s3
    /// - `obj = bucket.Object('key')` -> obj is s3
    fn track_resource_derived_variables(
        &mut self,
        root: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) {
        // First, track function-level assignments
        let func_def_pattern = "def $FUNC($$$PARAMS):$$$BODY";
        for func_match in root.find_all(func_def_pattern) {
            let env = func_match.get_env();

            let func_name = if let Some(node) = env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };

            // Find resource-derived assignments within this function body
            let pattern = "$VAR = $RESOURCE.$METHOD($$$ARGS)";
            for node_match in func_match.get_node().find_all(pattern) {
                let assign_env = node_match.get_env();

                let var_name = if let Some(node) = assign_env.get_match("VAR") {
                    node.text().to_string()
                } else {
                    continue;
                };

                let resource_name = if let Some(node) = assign_env.get_match("RESOURCE") {
                    node.text().to_string()
                } else {
                    continue;
                };

                let method_name = if let Some(node) = assign_env.get_match("METHOD") {
                    node.text().to_string()
                } else {
                    continue;
                };

                // Skip paginator and waiter methods - these are handled by separate extractors
                // (paginator_extractor.rs and waiters_extractor.rs)
                // Future work: Track paginator/waiter variables for cross-function usage
                if method_name == "get_paginator" || method_name == "get_waiter" {
                    continue;
                }

                // Skip if this variable is already tracked in this function scope
                // This prevents overwriting more specific kinds with the generic ResourceCollection
                if let Some(func_scope) = self.function_scopes.get(&func_name) {
                    if func_scope.contains_key(&var_name) {
                        continue;
                    }
                }

                // If we know the resource's service, track the derived variable
                // Check in function context first (for function-local resources)
                // Clone the type info to avoid borrow checker issues
                let type_info = self
                    .get_type_info_for_variable_in_context(&resource_name, Some(&func_name))
                    .cloned();

                if let Some(type_info) = type_info {
                    log::debug!(
                        "Tracked resource-derived variable in function '{}': {} from {}.{}() -> service '{}'",
                        func_name,
                        var_name,
                        resource_name,
                        method_name,
                        type_info.service_name
                    );
                    // Resource collections inherit the service from their parent resource
                    self.function_scopes
                        .entry(func_name.clone())
                        .or_default()
                        .insert(
                            var_name,
                            VariableTypeInfo::from_service_with_kind(
                                type_info.service_name.clone(),
                                SdkObjectKind::ResourceCollection,
                            ),
                        );
                }
            }
        }

        // Then track module-level assignments
        // This covers: table = dynamodb.Table('users'), bucket = s3.Bucket('name'), etc.
        let pattern = "$VAR = $RESOURCE.$METHOD($$$ARGS)";

        for node_match in root.find_all(pattern) {
            let env = node_match.get_env();

            // Check if this assignment is inside a function definition
            let mut current = node_match.get_node().parent();
            let mut is_inside_function = false;
            while let Some(node) = current {
                if node.kind() == node_kinds::FUNCTION_DEFINITION {
                    is_inside_function = true;
                    break;
                }
                current = node.parent();
            }

            if is_inside_function {
                continue; // Skip function-level assignments (already handled above)
            }

            let var_name = if let Some(node) = env.get_match("VAR") {
                node.text().to_string()
            } else {
                continue;
            };

            // Skip if this variable is already tracked
            // This prevents overwriting more specific kinds with the generic ResourceCollection
            if self.module_scope.contains_key(&var_name) {
                continue;
            }

            let resource_name = if let Some(node) = env.get_match("RESOURCE") {
                node.text().to_string()
            } else {
                continue;
            };

            let method_name = if let Some(node) = env.get_match("METHOD") {
                node.text().to_string()
            } else {
                continue;
            };

            // Skip paginator and waiter methods - these are handled by separate extractors
            // (paginator_extractor.rs and waiters_extractor.rs)
            // Future work: Track paginator/waiter variables for cross-function usage
            if method_name == "get_paginator" || method_name == "get_waiter" {
                continue;
            }

            // If we know the resource's service, track the derived variable
            // Common patterns:
            // - dynamodb.Table() -> dynamodb
            // - s3.Bucket() -> s3
            // - bucket.Object() -> s3 (if bucket is s3)
            if let Some(type_info) =
                self.get_type_info_for_variable_in_context(&resource_name, None)
            {
                log::debug!(
                    "Tracked resource-derived variable at module level: {} from {}.{}() -> service '{}'",
                    var_name,
                    resource_name,
                    method_name,
                    type_info.service_name
                );
                // Resource collections inherit the service from their parent resource
                self.module_scope.insert(
                    var_name,
                    VariableTypeInfo::from_service_with_kind(
                        type_info.service_name.clone(),
                        SdkObjectKind::ResourceCollection,
                    ),
                );
            }
        }
    }
    /// Extract string content from a string literal
    ///
    /// Handles both single and double quotes:
    /// - `'s3'` -> `s3`
    /// - `"dynamodb"` -> `dynamodb`
    fn extract_string_literal(&self, raw: &str) -> String {
        raw.trim()
            .trim_start_matches('\'')
            .trim_start_matches('"')
            .trim_end_matches('\'')
            .trim_end_matches('"')
            .to_string()
    }

    /// Look up the service type for a variable name
    ///
    /// Returns the service name if the variable is a tracked boto3 client/resource
    ///
    /// Checks in Python scoping order (LEGB - Local, Enclosing, Global, Built-in):
    /// 1. Function scope (local variables and parameters)
    /// 2. Module scope (global variables)
    ///
    /// # Parameters
    /// - `var_name`: The variable name to look up
    /// - `function_name`: Optional function context for function-scoped variables
    ///
    /// # Note on Parameter Types
    /// When checking parameter types, this returns the first service found in the set.
    /// For parameters with multiple possible types, use `get_services_for_parameter()`.
    pub(crate) fn get_service_for_variable_in_context(
        &self,
        var_name: &str,
        function_name: Option<&str>,
    ) -> Option<&String> {
        // First check function scope if we have a function context
        // This includes both local variables AND parameters (both are local scope in Python)
        if let Some(func_name) = function_name {
            // Check function-local variables first
            if let Some(func_scope) = self.function_scopes.get(func_name) {
                if let Some(type_info) = func_scope.get(var_name) {
                    return Some(&type_info.service_name);
                }
            }

            // Then check parameters (still part of local scope in Python)
            if let Some(type_infos) = self
                .parameter_types
                .get(&(func_name.to_string(), var_name.to_string()))
            {
                // Return first match for parameters
                return type_infos.iter().next().map(|info| &info.service_name);
            }
        }

        // Then check module scope (global variables)
        if let Some(type_info) = self.module_scope.get(var_name) {
            return Some(&type_info.service_name);
        }

        None
    }

    /// Get the full type information for a variable (not just service name)
    ///
    /// This is useful when you need access to the kind or qualified_type fields.
    ///
    /// Checks in Python scoping order (LEGB - Local, Enclosing, Global, Built-in):
    /// 1. Function scope (local variables and parameters)
    /// 2. Module scope (global variables)
    pub(crate) fn get_type_info_for_variable_in_context(
        &self,
        var_name: &str,
        function_name: Option<&str>,
    ) -> Option<&VariableTypeInfo> {
        // First check function scope if we have a function context
        // This includes both local variables AND parameters (both are local scope in Python)
        if let Some(func_name) = function_name {
            // Check function-local variables first
            if let Some(func_scope) = self.function_scopes.get(func_name) {
                if let Some(type_info) = func_scope.get(var_name) {
                    return Some(type_info);
                }
            }

            // Then check parameters (still part of local scope in Python)
            if let Some(type_infos) = self
                .parameter_types
                .get(&(func_name.to_string(), var_name.to_string()))
            {
                // Return first match for parameters
                if let Some(type_info) = type_infos.iter().next() {
                    return Some(type_info);
                }
            }
        }

        // Then check module scope (global variables)
        if let Some(type_info) = self.module_scope.get(var_name) {
            return Some(type_info);
        }

        None
    }

    /// Look up the service types for a function parameter
    ///
    /// Returns a set of possible service names if the parameter has been inferred from call sites.
    /// Multiple services are possible because Python allows the same function to be called
    /// with different argument types.
    ///
    /// Example:
    /// ```python
    /// def process(client):
    ///     client.put_object(...)
    ///
    /// s3 = boto3.client('s3')
    /// ec2 = boto3.client('ec2')
    /// process(s3)   # client → s3
    /// process(ec2)  # client → ec2
    /// # Returns: Some({"s3", "ec2"})
    /// ```
    pub(crate) fn get_services_for_parameter(
        &self,
        func_name: &str,
        param_name: &str,
    ) -> Option<HashSet<String>> {
        self.parameter_types
            .get(&(func_name.to_string(), param_name.to_string()))
            .map(|type_infos| {
                type_infos
                    .iter()
                    .map(|info| info.service_name.clone())
                    .collect()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SourceFile;
    use ast_grep_core::tree_sitter::LanguageExt;

    fn create_ast(source_code: &str) -> AstWithSourceFile<Python> {
        let source_file = SourceFile::with_language(
            std::path::PathBuf::new(),
            source_code.to_string(),
            crate::Language::Python,
        );
        let ast_grep = Python.ast_grep(&source_file.content);
        AstWithSourceFile::new(ast_grep, source_file)
    }

    // Test helper: convenience method that calls get_service_for_variable_in_context with no function context
    impl VariableTypeTracker {
        fn get_service_for_variable(&self, var_name: &str) -> Option<&String> {
            self.get_service_for_variable_in_context(var_name, None)
        }
    }

    // ========== Basic Assignment Tracking Tests ==========

    #[test]
    fn test_track_simple_client_assignment() {
        let source_code = r#"
import boto3
s3_client = boto3.client('s3')
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        assert_eq!(
            tracker.get_service_for_variable("s3_client"),
            Some(&"s3".to_string())
        );
    }

    #[test]
    fn test_track_simple_resource_assignment() {
        let source_code = r#"
import boto3
dynamodb = boto3.resource('dynamodb')
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        assert_eq!(
            tracker.get_service_for_variable("dynamodb"),
            Some(&"dynamodb".to_string())
        );
    }

    #[test]
    fn test_track_multiple_assignments() {
        let source_code = r#"
import boto3
s3_client = boto3.client('s3')
ec2_client = boto3.client('ec2')
dynamodb = boto3.resource('dynamodb')
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        assert_eq!(
            tracker.get_service_for_variable("s3_client"),
            Some(&"s3".to_string())
        );
        assert_eq!(
            tracker.get_service_for_variable("ec2_client"),
            Some(&"ec2".to_string())
        );
        assert_eq!(
            tracker.get_service_for_variable("dynamodb"),
            Some(&"dynamodb".to_string())
        );
    }

    #[test]
    fn test_double_quotes() {
        let source_code = r#"
import boto3
s3_client = boto3.client("s3")
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        assert_eq!(
            tracker.get_service_for_variable("s3_client"),
            Some(&"s3".to_string())
        );
    }

    #[test]
    fn test_unknown_variable() {
        let source_code = r#"
import boto3
s3_client = boto3.client('s3')
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        assert_eq!(tracker.get_service_for_variable("unknown_var"), None);
    }

    #[test]
    fn test_real_world_scenario() {
        let source_code = r#"
import boto3

s3_direct = boto3.client('s3')
s3_direct.put_object(Bucket='bucket1', Key='key1', Body=b'data1')

def upload_data(client, bucket, key):
    client.put_object(Bucket=bucket, Key=key, Body=b'data2')

s3_client = boto3.client('s3')
upload_data(s3_client, 'bucket2', 'key2')

dynamodb_direct = boto3.resource('dynamodb')
table_direct = dynamodb_direct.Table('users')
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        // Should track both client and resource assignments
        assert_eq!(
            tracker.get_service_for_variable("s3_direct"),
            Some(&"s3".to_string())
        );
        assert_eq!(
            tracker.get_service_for_variable("s3_client"),
            Some(&"s3".to_string())
        );
        assert_eq!(
            tracker.get_service_for_variable("dynamodb_direct"),
            Some(&"dynamodb".to_string())
        );
    }

    // ========== Alias Tracking Tests ==========

    #[test]
    fn test_simple_alias() {
        let source_code = r#"
import boto3
s3_client = boto3.client('s3')
my_client = s3_client
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        assert_eq!(
            tracker.get_service_for_variable("s3_client"),
            Some(&"s3".to_string())
        );
        assert_eq!(
            tracker.get_service_for_variable("my_client"),
            Some(&"s3".to_string())
        );
    }

    // ========== Function Parameter Inference Tests ==========

    #[test]
    fn test_function_parameter_inference() {
        let source_code = r#"
import boto3

def upload_file(client):
    pass

s3_client = boto3.client('s3')
upload_file(s3_client)
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        // Should infer that 'client' parameter in upload_file is s3
        let services = tracker.get_services_for_parameter("upload_file", "client");
        assert!(services.is_some());
        assert!(services.unwrap().contains("s3"));
    }

    #[test]
    fn test_function_parameter_multiple_types() {
        // This is the critical test case: same function called with different service types
        let source_code = r#"
import boto3

def process_data(client):
    # This function is called with both S3 and EC2 clients
    # We should track BOTH possible types
    pass

s3_client = boto3.client('s3')
ec2_client = boto3.client('ec2')

process_data(s3_client)
process_data(ec2_client)
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        // Should track BOTH s3 and ec2 as possible types for 'client' parameter
        let services = tracker.get_services_for_parameter("process_data", "client");
        assert!(services.is_some());
        let services = services.unwrap();
        assert_eq!(services.len(), 2);
        assert!(services.contains("s3"));
        assert!(services.contains("ec2"));
    }

    #[test]
    fn test_multiple_function_calls() {
        let source_code = r#"
import boto3

def process_s3(client):
    pass

def process_dynamodb(table):
    pass

s3 = boto3.client('s3')
dynamodb = boto3.resource('dynamodb')

process_s3(s3)
process_dynamodb(dynamodb)
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        let s3_services = tracker.get_services_for_parameter("process_s3", "client");
        assert!(s3_services.is_some());
        assert!(s3_services.unwrap().contains("s3"));

        let dynamodb_services = tracker.get_services_for_parameter("process_dynamodb", "table");
        assert!(dynamodb_services.is_some());
        assert!(dynamodb_services.unwrap().contains("dynamodb"));
    }

    #[test]
    fn test_chained_aliases() {
        let source_code = r#"
import boto3
s3_client = boto3.client('s3')
client_a = s3_client
client_b = client_a
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        assert_eq!(
            tracker.get_service_for_variable("s3_client"),
            Some(&"s3".to_string())
        );
        assert_eq!(
            tracker.get_service_for_variable("client_a"),
            Some(&"s3".to_string())
        );
        assert_eq!(
            tracker.get_service_for_variable("client_b"),
            Some(&"s3".to_string())
        );
    }

    #[test]
    fn test_multiple_parameters() {
        let source_code = r#"
import boto3

def sync_data(s3_client, dynamodb_client):
    s3_client.get_object(Bucket='bucket', Key='key')
    dynamodb_client.put_item(TableName='table', Item={})

s3 = boto3.client('s3')
dynamodb = boto3.client('dynamodb')
sync_data(s3, dynamodb)
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        // Debug: print what we tracked
        println!("Module scope: {:?}", tracker.module_scope);
        println!("Parameter types: {:?}", tracker.parameter_types);

        // Should track both parameters
        let s3_services = tracker.get_services_for_parameter("sync_data", "s3_client");
        println!("s3_services: {:?}", s3_services);
        assert!(
            s3_services.is_some(),
            "s3_client parameter should be tracked"
        );
        assert!(s3_services.unwrap().contains("s3"));

        let dynamodb_services = tracker.get_services_for_parameter("sync_data", "dynamodb_client");
        println!("dynamodb_services: {:?}", dynamodb_services);
        assert!(
            dynamodb_services.is_some(),
            "dynamodb_client parameter should be tracked"
        );
        assert!(dynamodb_services.unwrap().contains("dynamodb"));
    }

    #[test]
    fn test_three_parameters() {
        let source_code = r#"
import boto3

def process(s3, ec2, lambda_client):
    s3.list_buckets()
    ec2.describe_instances()
    lambda_client.list_functions()

s3 = boto3.client('s3')
ec2 = boto3.client('ec2')
lambda_client = boto3.client('lambda')
process(s3, ec2, lambda_client)
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        // All three parameters should be tracked
        let s3_services = tracker.get_services_for_parameter("process", "s3");
        assert!(s3_services.is_some());
        assert!(s3_services.unwrap().contains("s3"));

        let ec2_services = tracker.get_services_for_parameter("process", "ec2");
        assert!(ec2_services.is_some());
        assert!(ec2_services.unwrap().contains("ec2"));

        let lambda_services = tracker.get_services_for_parameter("process", "lambda_client");
        assert!(lambda_services.is_some());
        assert!(lambda_services.unwrap().contains("lambda"));
    }

    #[test]
    fn test_mixed_parameters() {
        let source_code = r#"
import boto3

def upload(client, bucket_name, key):
    # Only 'client' is a tracked boto3 client
    # bucket_name and key are strings (not tracked)
    client.put_object(Bucket=bucket_name, Key=key, Body=b'data')

s3 = boto3.client('s3')
upload(s3, 'my-bucket', 'my-key')
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        // Only 'client' parameter should be tracked
        let client_services = tracker.get_services_for_parameter("upload", "client");
        assert!(client_services.is_some());
        assert!(client_services.unwrap().contains("s3"));

        // bucket_name and key should not be tracked (they're not boto3 clients)
        assert!(tracker
            .get_services_for_parameter("upload", "bucket_name")
            .is_none());
        assert!(tracker
            .get_services_for_parameter("upload", "key")
            .is_none());
    }

    // ========== Helper Function Tests ==========

    #[test]
    fn test_extract_all_params() {
        // Test basic cases
        assert_eq!(
            VariableTypeTracker::extract_all_params("client"),
            vec!["client"]
        );
        assert_eq!(
            VariableTypeTracker::extract_all_params("client, bucket, key"),
            vec!["client", "bucket", "key"]
        );
        assert_eq!(
            VariableTypeTracker::extract_all_params("self, client, table"),
            vec!["self", "client", "table"]
        );
        assert_eq!(
            VariableTypeTracker::extract_all_params(""),
            Vec::<String>::new()
        );

        // Test with default values
        assert_eq!(
            VariableTypeTracker::extract_all_params("client=None, bucket='default'"),
            vec!["client", "bucket"]
        );

        // Test with type annotations
        assert_eq!(
            VariableTypeTracker::extract_all_params("client: str, count: int"),
            vec!["client", "count"]
        );

        // Test with whitespace
        assert_eq!(
            VariableTypeTracker::extract_all_params(" client , bucket "),
            vec!["client", "bucket"]
        );

        // Test mixed
        assert_eq!(
            VariableTypeTracker::extract_all_params("client, bucket='default', key: str"),
            vec!["client", "bucket", "key"]
        );
    }

    // ========== SDK Object Kind Inference Tests ==========

    #[test]
    fn test_client_kind_inference() {
        let source_code = r#"
import boto3
s3_client = boto3.client('s3')
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        let info = tracker.get_type_info_for_variable_in_context("s3_client", None);
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.service_name, "s3");
        assert_eq!(info.kind, Some(SdkObjectKind::Client));
        assert_eq!(info.qualified_type, None);
    }

    #[test]
    fn test_resource_kind_inference() {
        let source_code = r#"
import boto3
s3 = boto3.resource('s3')
dynamodb = boto3.resource('dynamodb')
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        let s3_info = tracker.get_type_info_for_variable_in_context("s3", None);
        assert!(s3_info.is_some());
        let s3_info = s3_info.unwrap();
        assert_eq!(s3_info.service_name, "s3");
        assert_eq!(s3_info.kind, Some(SdkObjectKind::Resource));

        let dynamodb_info = tracker.get_type_info_for_variable_in_context("dynamodb", None);
        assert!(dynamodb_info.is_some());
        let dynamodb_info = dynamodb_info.unwrap();
        assert_eq!(dynamodb_info.service_name, "dynamodb");
        assert_eq!(dynamodb_info.kind, Some(SdkObjectKind::Resource));
    }

    #[test]
    fn test_resource_collection_kind_inference() {
        let source_code = r#"
import boto3
dynamodb = boto3.resource('dynamodb')
table = dynamodb.Table('users')
s3 = boto3.resource('s3')
bucket = s3.Bucket('my-bucket')
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        let dynamodb_info = tracker.get_type_info_for_variable_in_context("dynamodb", None);
        assert!(dynamodb_info.is_some());
        assert_eq!(dynamodb_info.unwrap().kind, Some(SdkObjectKind::Resource));

        let table_info = tracker.get_type_info_for_variable_in_context("table", None);
        assert!(table_info.is_some());
        let table_info = table_info.unwrap();
        assert_eq!(table_info.service_name, "dynamodb");
        assert_eq!(table_info.kind, Some(SdkObjectKind::ResourceCollection));

        let s3_info = tracker.get_type_info_for_variable_in_context("s3", None);
        assert!(s3_info.is_some());
        assert_eq!(s3_info.unwrap().kind, Some(SdkObjectKind::Resource));

        let bucket_info = tracker.get_type_info_for_variable_in_context("bucket", None);
        assert!(bucket_info.is_some());
        let bucket_info = bucket_info.unwrap();
        assert_eq!(bucket_info.service_name, "s3");
        assert_eq!(bucket_info.kind, Some(SdkObjectKind::ResourceCollection));
    }

    #[test]
    fn test_kind_preserved_through_aliases() {
        let source_code = r#"
import boto3
s3_client = boto3.client('s3')
my_client = s3_client
another_client = my_client
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        // All aliases should preserve the Client kind
        let s3_info = tracker.get_type_info_for_variable_in_context("s3_client", None);
        assert!(s3_info.is_some());
        assert_eq!(s3_info.unwrap().kind, Some(SdkObjectKind::Client));

        let my_info = tracker.get_type_info_for_variable_in_context("my_client", None);
        assert!(my_info.is_some());
        assert_eq!(my_info.unwrap().kind, Some(SdkObjectKind::Client));

        let another_info = tracker.get_type_info_for_variable_in_context("another_client", None);
        assert!(another_info.is_some());
        assert_eq!(another_info.unwrap().kind, Some(SdkObjectKind::Client));
    }

    #[test]
    fn test_service_name_and_kind_apis() {
        // Test both APIs: get_service_for_variable (service name only) and
        // get_type_info_for_variable_in_context (full type info including kind)
        let source_code = r#"
import boto3
s3_client = boto3.client('s3')
s3_resource = boto3.resource('s3')
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        // Simple API returns service name only
        assert_eq!(
            tracker.get_service_for_variable("s3_client"),
            Some(&"s3".to_string())
        );
        assert_eq!(
            tracker.get_service_for_variable("s3_resource"),
            Some(&"s3".to_string())
        );

        // Full API provides type info including kind
        let client_info = tracker.get_type_info_for_variable_in_context("s3_client", None);
        assert!(client_info.is_some());
        assert_eq!(client_info.unwrap().kind, Some(SdkObjectKind::Client));

        let resource_info = tracker.get_type_info_for_variable_in_context("s3_resource", None);
        assert!(resource_info.is_some());
        assert_eq!(resource_info.unwrap().kind, Some(SdkObjectKind::Resource));
    }

    #[test]
    fn test_client_vs_resource_distinction() {
        // This is the key test: verify we can distinguish client from resource
        let source_code = r#"
import boto3
s3_client = boto3.client('s3')
s3_resource = boto3.resource('s3')
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        let client_info = tracker
            .get_type_info_for_variable_in_context("s3_client", None)
            .unwrap();
        let resource_info = tracker
            .get_type_info_for_variable_in_context("s3_resource", None)
            .unwrap();

        // Both are S3, but different kinds
        assert_eq!(client_info.service_name, "s3");
        assert_eq!(resource_info.service_name, "s3");

        // This is the critical distinction
        assert_eq!(client_info.kind, Some(SdkObjectKind::Client));
        assert_eq!(resource_info.kind, Some(SdkObjectKind::Resource));
        assert_ne!(client_info.kind, resource_info.kind);
    }

    // ========== Python Scoping (LEGB) Tests ==========

    #[test]
    fn test_parameter_shadows_module_variable() {
        // Critical test: Parameters should shadow module-level variables (Python LEGB scoping)
        let source_code = r#"
import boto3

# Module-level variable
s3_client = boto3.client('s3')

def upload_file(s3_client):
    # Parameter 's3_client' shadows module-level 's3_client'
    # Inside this function, s3_client should resolve to the parameter type, not module type
    pass

# Call with different service
dynamodb_client = boto3.client('dynamodb')
upload_file(dynamodb_client)
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        // Module-level s3_client should be 's3'
        assert_eq!(
            tracker.get_service_for_variable_in_context("s3_client", None),
            Some(&"s3".to_string())
        );

        // Inside upload_file, s3_client parameter should be 'dynamodb' (from call site)
        // This tests that parameters shadow module-level variables
        let param_service =
            tracker.get_service_for_variable_in_context("s3_client", Some("upload_file"));
        assert_eq!(param_service, Some(&"dynamodb".to_string()));

        // Verify it's different from module-level
        let module_service = tracker.get_service_for_variable_in_context("s3_client", None);
        assert_ne!(param_service, module_service);
    }

    #[test]
    fn test_function_variable_shadows_module_variable() {
        // Test that function-local variables shadow module-level variables (Python LEGB scoping)
        let source_code = r#"
import boto3

# Module-level variable
client = boto3.client('s3')

def process_data():
    # Function-local variable shadows module-level
    client = boto3.client('dynamodb')
    client.put_item(TableName='table', Item={})
"#;
        let ast = create_ast(source_code);
        let mut tracker = VariableTypeTracker::new();
        tracker.track_boto3_assignments(&ast);

        // Debug: print what we tracked
        println!("Module scope: {:?}", tracker.module_scope);
        println!("Function scopes: {:?}", tracker.function_scopes);

        // Module-level client should be 's3'
        assert_eq!(
            tracker.get_service_for_variable_in_context("client", None),
            Some(&"s3".to_string())
        );

        // Inside process_data, function-local client should be 'dynamodb' (shadows module-level)
        assert_eq!(
            tracker.get_service_for_variable_in_context("client", Some("process_data")),
            Some(&"dynamodb".to_string())
        );

        // Verify they are different (shadowing works correctly)
        let module_service = tracker.get_service_for_variable_in_context("client", None);
        let function_service =
            tracker.get_service_for_variable_in_context("client", Some("process_data"));
        assert_ne!(module_service, function_service);
    }
}
