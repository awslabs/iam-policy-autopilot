//! Resource direct call extraction for Python AWS SDK using ast-grep
//!
//! This module handles extraction of boto3 resource-based patterns using authoritative
//! boto3 resources JSON specifications with a three-tier evidence-based approach:
//!
//! **Tier 1 (Precision)**: Known receiver + matched boto3 method → extract only matched calls
//!
//! **Tier 2 (Conservative with Evidence)**:
//!   - Known receiver + utility method → expand to underlying operations
//!   - Known receiver + collection access (hasMany) → generate collection synthetic
//!   - Known receiver + unmatched method → add all synthetic operations pointing to unmatched call
//!
//! **Tier 3 (Service-Agnostic Fallback)**:
//!   - Unknown receiver + utility method → search all services for matching utility methods
//!   - Unknown receiver + collection access → search all services for hasMany collections
//!   - Position-based deduplication ensures no overlap with Tier 1/2 extractions
//!
//! Example patterns:
//! ```python
//! # Tier 1: Known receiver + matched method
//! table = dynamodb.Table('my-table')
//! table.get_item(Key={'id': 1})  # Matched action → precise extraction
//!
//! # Tier 2: Known receiver + utility method
//! bucket = s3.Bucket('my-bucket')
//! bucket.upload_file('file', 'key')  # Utility method → expands to put_object + others
//!
//! # Tier 2: Known receiver + collection access
//! bucket = s3.Bucket('my-bucket')
//! objects = bucket.objects  # hasMany collection → list_objects(Bucket='my-bucket')
//!
//! # Tier 3: Unknown receiver (cross-file reference or function parameter)
//! unknown_bucket.upload_file('x', 'y')  # Conservative → synthetics for all S3 operations
//! unknown_var.objects  # Conservative → synthetics for all services with 'objects' collection
//! ```

use crate::extraction::python::boto3_resources_model::{
    Boto3ResourcesModel, Boto3ResourcesRegistry, HasManySpec, OperationType,
};
use crate::extraction::python::common::ArgumentExtractor;
use crate::extraction::{Parameter, ParameterValue, SdkMethodCall, SdkMethodCallMetadata};
use crate::ServiceModelIndex;
use ast_grep_language::Python;
use convert_case::{Case, Casing};
use std::collections::{HashMap, HashSet};

/// Position tracking for deduplication (Tier 3)
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct MatchedPosition {
    line: usize,
    column: usize,
}

/// Information about a discovered resource constructor call
#[derive(Debug, Clone)]
struct ResourceConstructorInfo {
    variable_name: String,
    resource_type: String,
    service_name: String,
    constructor_args: Vec<Parameter>,
    #[allow(dead_code)]
    start_position: (usize, usize),
    #[allow(dead_code)]
    end_position: (usize, usize),
}

/// Information about a method call on a resource object
#[derive(Debug, Clone)]
struct ResourceMethodCallInfo {
    resource_var: String,
    method_name: String,
    arguments: Vec<Parameter>,
    method_call_line: usize,
    start_position: (usize, usize),
    end_position: (usize, usize),
}

/// Resource usage classification for two-tier approach (Tier 3 is now separate)
#[derive(Debug, Clone)]
enum ResourceUsage {
    /// All method calls matched to boto3 specs (Tier 1)
    FullyMatched { matched_calls: Vec<SdkMethodCall> },
    /// Has at least one unmatched method (Tier 2)
    HasUnmatchedMethod {
        matched_calls: Vec<SdkMethodCall>,
        unmatched_method: ResourceMethodCallInfo,
    },
}

/// Tracking structure for resource analysis
#[derive(Debug)]
struct ResourceAnalysis {
    constructor: ResourceConstructorInfo,
    usage: ResourceUsage,
}

/// Evidence source for synthetic call generation
#[derive(Debug, Clone)]
enum SyntheticEvidenceSource {
    /// Use position from unmatched method call
    UnmatchedMethod(ResourceMethodCallInfo),
}

/// Extractor for boto3 resource direct call patterns
pub(crate) struct ResourceDirectCallsExtractor<'a> {
    registry: Boto3ResourcesRegistry,
    service_index: &'a ServiceModelIndex,
}

impl<'a> ResourceDirectCallsExtractor<'a> {
    /// Create a new resource direct calls extractor with ServiceModelIndex access
    pub(crate) fn new(service_index: &'a ServiceModelIndex) -> Self {
        let registry = Boto3ResourcesRegistry::load_common_services_with_utilities();
        Self {
            registry,
            service_index,
        }
    }

    /// Extract resource direct call method calls using three-tier evidence-based approach
    ///
    /// **Tier 1**: Fully matched methods → precise extraction
    /// **Tier 2**: Has unmatched methods → conservative with unmatched position as evidence
    /// **Tier 3**: Constructor only → maximum conservation with constructor position as evidence
    pub(crate) fn extract_resource_method_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Vec<SdkMethodCall> {
        // Step 1: Find all resource constructors using service-agnostic matching
        let constructors = self.find_resource_constructors(ast, &self.registry);

        // Step 2: Find all method calls on resource objects
        let method_calls = self.find_resource_method_calls(ast);

        // Step 3: Analyze each resource's usage pattern
        let resource_analyses =
            self.analyze_resource_usage(&constructors, &method_calls, self.registry.models());

        // Step 4: Generate appropriate calls based on usage tier
        let mut all_calls = Vec::new();

        for analysis in resource_analyses {
            let boto3_model = match self.registry.get_model(&analysis.constructor.service_name) {
                Some(model) => model,
                None => continue,
            };

            match analysis.usage {
                ResourceUsage::FullyMatched { matched_calls } => {
                    // Tier 1: Add only matched calls (already have correct positions)
                    all_calls.extend(matched_calls);
                }
                ResourceUsage::HasUnmatchedMethod {
                    matched_calls,
                    unmatched_method,
                } => {
                    // Tier 2: Add matched + all synthetic (using unmatched position)
                    all_calls.extend(matched_calls);

                    let synthetics = self.generate_synthetic_calls_for_resource(
                        &analysis.constructor,
                        boto3_model,
                        SyntheticEvidenceSource::UnmatchedMethod(unmatched_method),
                    );
                    all_calls.extend(synthetics);
                }
            }
        }

        // Step 5: Find and generate synthetic calls for hasMany collections (Tier 2 approach)
        let collection_synthetics =
            self.find_and_generate_collection_synthetics(ast, &constructors);
        all_calls.extend(collection_synthetics);

        // Step 6: Collect matched positions for Tier 3 deduplication
        let mut matched_positions = HashSet::new();
        for call in &all_calls {
            if let Some(metadata) = &call.metadata {
                matched_positions.insert(MatchedPosition {
                    line: metadata.start_position.0,
                    column: metadata.start_position.1,
                });
            }
        }

        // Step 7: New Tier 3 - service-agnostic fallback for unknown receivers
        let tier3_calls = self.find_unmatched_utility_and_collection_calls(ast, &matched_positions);
        all_calls.extend(tier3_calls);

        all_calls
    }

    /// Analyze resource usage to classify into three tiers
    fn analyze_resource_usage(
        &self,
        constructors: &[ResourceConstructorInfo],
        method_calls: &[ResourceMethodCallInfo],
        boto3_models: &HashMap<String, Boto3ResourcesModel>,
    ) -> Vec<ResourceAnalysis> {
        let mut analyses = Vec::new();

        for constructor in constructors {
            // Find all method calls for this resource
            let resource_methods: Vec<_> = method_calls
                .iter()
                .filter(|mc| mc.resource_var == constructor.variable_name)
                .collect();

            if resource_methods.is_empty() {
                // Skip constructors with no method calls - these will be handled by new Tier 3
                continue;
            }

            // Try to match each method call
            let boto3_model = match boto3_models.get(&constructor.service_name) {
                Some(model) => model,
                None => continue,
            };

            let mut matched_calls = Vec::new();
            let mut first_unmatched = None;

            for method_call in resource_methods {
                if let Some(call) = self.try_match_method(method_call, constructor, boto3_model) {
                    matched_calls.push(call);
                    continue;
                }

                let utility_calls =
                    self.try_expand_resource_utility_method(method_call, constructor, boto3_model);
                if !utility_calls.is_empty() {
                    matched_calls.extend(utility_calls);
                    continue;
                }

                if first_unmatched.is_none() {
                    first_unmatched = Some((*method_call).clone());
                }
            }

            let usage = if let Some(unmatched) = first_unmatched {
                // Tier 2: Has unmatched method
                ResourceUsage::HasUnmatchedMethod {
                    matched_calls,
                    unmatched_method: unmatched,
                }
            } else {
                // Tier 1: Fully matched
                ResourceUsage::FullyMatched { matched_calls }
            };

            analyses.push(ResourceAnalysis {
                constructor: constructor.clone(),
                usage,
            });
        }

        analyses
    }

    /// Try to match a method call to boto3 actions or collections
    fn try_match_method(
        &self,
        method_call: &ResourceMethodCallInfo,
        constructor: &ResourceConstructorInfo,
        boto3_model: &Boto3ResourcesModel,
    ) -> Option<SdkMethodCall> {
        // Try to match to action first
        if let Some(call) = self.create_synthetic_method_call_with_waiter_resolution(
            method_call,
            constructor,
            boto3_model,
        ) {
            return Some(call);
        }

        None
    }

    /// Try to expand a resource utility method into underlying SDK operations
    /// Returns a vector of SDK calls (utility methods expand to multiple operations)
    fn try_expand_resource_utility_method(
        &self,
        method_call: &ResourceMethodCallInfo,
        constructor: &ResourceConstructorInfo,
        boto3_model: &Boto3ResourcesModel,
    ) -> Vec<SdkMethodCall> {
        // Check if this is a resource utility method
        let utility_method = match boto3_model
            .get_resource_utility_method(&constructor.resource_type, &method_call.method_name)
        {
            Some(method) => method,
            None => return Vec::new(),
        };

        let mut expanded_calls = Vec::new();

        // Expand into each operation
        for operation in &utility_method.operations {
            let mut parameters = Vec::new();

            // Inject identifier parameters from constructor based on identifier_mappings
            for id_mapping in &utility_method.identifier_mappings {
                if let Some(constructor_arg) = constructor
                    .constructor_args
                    .get(id_mapping.constructor_arg_index)
                {
                    let value = match constructor_arg {
                        Parameter::Positional { value, .. } => value.clone(),
                        Parameter::Keyword { value, .. } => value.clone(),
                        Parameter::DictionarySplat { expression, .. } => {
                            ParameterValue::Unresolved(expression.clone())
                        }
                    };

                    parameters.push(Parameter::Keyword {
                        name: id_mapping.target_param.clone(),
                        value,
                        position: parameters.len(),
                        type_annotation: None,
                    });
                }
            }

            // Add method call arguments (positional mapping from utility method spec)
            // Only add parameters that are actually needed by the operation
            for (arg_index, param) in method_call.arguments.iter().enumerate() {
                // Map positional arguments using accepted_params
                let param_to_add = if let Parameter::Positional {
                    value,
                    type_annotation,
                    ..
                } = param
                {
                    // For positional args, map to keyword args using accepted_params
                    if let Some(param_name) = utility_method.accepted_params.get(arg_index) {
                        // Only add this parameter if it's needed by the operation
                        if operation.required_params.contains(param_name) {
                            Parameter::Keyword {
                                name: param_name.clone(),
                                value: value.clone(),
                                position: parameters.len(),
                                type_annotation: type_annotation.clone(),
                            }
                        } else {
                            continue; // Skip parameters not needed by this operation
                        }
                    } else {
                        // Fallback: keep as positional
                        Parameter::Positional {
                            value: value.clone(),
                            position: parameters.len(),
                            type_annotation: type_annotation.clone(),
                            struct_fields: None,
                        }
                    }
                } else {
                    // Keyword and dictionary splat args pass through
                    match param {
                        Parameter::Keyword {
                            name,
                            value,
                            type_annotation,
                            ..
                        } => {
                            // Check if keyword args are needed by the operation
                            if operation.required_params.contains(name) {
                                Parameter::Keyword {
                                    name: name.clone(),
                                    value: value.clone(),
                                    position: parameters.len(),
                                    type_annotation: type_annotation.clone(),
                                }
                            } else {
                                continue; // Skip parameters not needed
                            }
                        }
                        Parameter::DictionarySplat { expression, .. } => {
                            Parameter::DictionarySplat {
                                expression: expression.clone(),
                                position: parameters.len(),
                            }
                        }
                        _ => continue,
                    }
                };

                parameters.push(param_to_add);
            }

            // Handle missing required parameters by generating synthetic values
            for required_param in &operation.required_params {
                // Check if parameter already exists
                let param_exists = parameters.iter().any(
                    |p| matches!(p, Parameter::Keyword { name, .. } if name == required_param),
                );

                if !param_exists {
                    parameters.push(Parameter::Keyword {
                        name: required_param.clone(),
                        value: ParameterValue::Unresolved(format!(
                            "synthetic_{}",
                            required_param.to_case(Case::Snake)
                        )),
                        position: parameters.len(),
                        type_annotation: None,
                    });
                }
            }

            expanded_calls.push(SdkMethodCall {
                name: operation.operation.to_case(Case::Snake),
                possible_services: vec![constructor.service_name.clone()],
                metadata: Some(SdkMethodCallMetadata {
                    parameters,
                    return_type: None,
                    start_position: method_call.start_position,
                    end_position: method_call.end_position,
                    receiver: Some(method_call.resource_var.clone()),
                }),
            });
        }

        expanded_calls
    }

    /// Generate synthetic calls for all resource operations
    fn generate_synthetic_calls_for_resource(
        &self,
        constructor: &ResourceConstructorInfo,
        boto3_model: &Boto3ResourcesModel,
        evidence: SyntheticEvidenceSource,
    ) -> Vec<SdkMethodCall> {
        let resource_def = match boto3_model.get_resource_definition(&constructor.resource_type) {
            Some(def) => def,
            None => return Vec::new(),
        };

        let mut synthetic_calls = Vec::new();

        // Extract position from evidence source
        let (start_pos, end_pos) = match evidence {
            SyntheticEvidenceSource::UnmatchedMethod(ref method_call) => {
                (method_call.start_position, method_call.end_position)
            }
        };

        // Generate synthetic call for each action
        for action_mapping in resource_def.actions.values() {
            let mut parameters = Vec::new();

            // Inject identifier parameters from constructor
            for param_mapping in &action_mapping.identifier_params {
                if let Some(param_name) = &param_mapping.name {
                    if let Some(_identifier) = resource_def
                        .identifiers
                        .iter()
                        .find(|id| id.name == *param_name)
                    {
                        // Get value from constructor args (first positional arg)
                        if let Some(first_arg) = constructor.constructor_args.first() {
                            let value = match first_arg {
                                Parameter::Positional { value, .. } => value.clone(),
                                Parameter::Keyword { value, .. } => value.clone(),
                                Parameter::DictionarySplat { expression, .. } => {
                                    ParameterValue::Unresolved(expression.clone())
                                }
                            };

                            parameters.push(Parameter::Keyword {
                                name: param_mapping.target.clone(),
                                value,
                                position: parameters.len(),
                                type_annotation: None,
                            });
                        }
                    }
                }
            }

            // Extract operation name using pattern matching
            let method_name = match &action_mapping.operation {
                OperationType::Waiter { waiter_name } => {
                    // For waiters in synthetic generation, use the waiter name directly
                    // The actual resolution happens in create_synthetic_method_call_with_waiter_resolution
                    format!("wait_until_{}", waiter_name.to_case(Case::Snake))
                }
                OperationType::SdkOperation(op_name) | OperationType::Load(op_name) => {
                    op_name.to_case(Case::Snake)
                }
            };

            synthetic_calls.push(SdkMethodCall {
                name: method_name,
                possible_services: vec![constructor.service_name.clone()],
                metadata: Some(SdkMethodCallMetadata {
                    parameters,
                    return_type: None,
                    start_position: start_pos,
                    end_position: end_pos,
                    receiver: Some(constructor.variable_name.clone()), // Use actual variable name from constructor
                }),
            });
        }

        synthetic_calls
    }

    /// Find all resource constructor calls in the AST using service-agnostic matching
    fn find_resource_constructors(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
        registry: &Boto3ResourcesRegistry,
    ) -> Vec<ResourceConstructorInfo> {
        let root = ast.root();
        let mut constructors = Vec::new();

        // Service-agnostic pattern: $VAR = $ANY.$RESOURCE_TYPE($$$ARGS)
        // This matches ANY object calling a method, regardless of how the service was instantiated
        let constructor_pattern = "$VAR = $ANY.$RESOURCE_TYPE($$$ARGS)";

        for node_match in root.find_all(constructor_pattern) {
            let env = node_match.get_env();

            // Extract variable name
            let variable_name = match env.get_match("VAR") {
                Some(node) => node.text().to_string(),
                None => continue,
            };

            // Extract resource type (e.g., "Table", "Bucket")
            let resource_type = match env.get_match("RESOURCE_TYPE") {
                Some(node) => node.text().to_string(),
                None => continue,
            };

            // Look up which services provide this resource type
            let possible_services = registry.find_services_for_resource(&resource_type);

            if possible_services.is_empty() {
                continue; // Not a known resource type
            }

            // Extract arguments
            let args_nodes = env.get_multiple_matches("ARGS");
            let constructor_args = ArgumentExtractor::extract_arguments(&args_nodes);

            // Get position information
            let node = node_match.get_node();
            let start = node.start_pos();
            let end = node.end_pos();

            // Create constructor info for EACH possible service
            for service_name in possible_services {
                if let Some(model) = registry.get_model(&service_name) {
                    if let Some(constructor_spec) = model.get_constructor_spec(&resource_type) {
                        // VALIDATION: Verify exact argument count matches expected identifiers
                        // Resource identifiers are always required in boto3 - they uniquely
                        // identify the resource instance and cannot be optional.
                        // The number of constructor arguments must equal the number of identifiers.
                        let expected_arg_count = constructor_spec.identifiers_count;
                        if constructor_args.len() != expected_arg_count {
                            continue; // Skip - invalid constructor call
                        }

                        constructors.push(ResourceConstructorInfo {
                            variable_name: variable_name.clone(),
                            resource_type: constructor_spec.resource_type.clone(),
                            service_name: service_name.clone(),
                            constructor_args: constructor_args.clone(),
                            start_position: (start.line() + 1, start.column(node) + 1),
                            end_position: (end.line() + 1, end.column(node) + 1),
                        });
                    }
                }
            }
        }

        constructors
    }

    /// Find all method calls on potential resource objects
    fn find_resource_method_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Vec<ResourceMethodCallInfo> {
        let root = ast.root();
        let mut method_calls = Vec::new();

        let method_call_pattern = "$RESULT = $RESOURCE_VAR.$METHOD($$$ARGS)";

        for node_match in root.find_all(method_call_pattern) {
            if let Some(method_call_info) = self.parse_resource_method_call(&node_match) {
                method_calls.push(method_call_info);
            }
        }

        // Also handle calls without assignment
        let simple_method_pattern = "$RESOURCE_VAR.$METHOD($$$ARGS)";

        for node_match in root.find_all(simple_method_pattern) {
            if let Some(method_call_info) = self.parse_simple_resource_method_call(&node_match) {
                method_calls.push(method_call_info);
            }
        }

        // Deduplicate method calls by (resource_var, method_name, line_number)
        method_calls.sort_by(|a, b| {
            a.resource_var
                .cmp(&b.resource_var)
                .then(a.method_name.cmp(&b.method_name))
                .then(a.method_call_line.cmp(&b.method_call_line))
        });
        method_calls.dedup_by(|a, b| {
            a.resource_var == b.resource_var
                && a.method_name == b.method_name
                && a.method_call_line == b.method_call_line
        });

        method_calls
    }

    /// Parse a resource method call (with assignment)
    fn parse_resource_method_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Option<ResourceMethodCallInfo> {
        let env = node_match.get_env();

        // Extract resource variable name
        let resource_var = env.get_match("RESOURCE_VAR")?.text().to_string();

        // Extract method name
        let method_name = env.get_match("METHOD")?.text().to_string();

        // Extract arguments
        let args_nodes = env.get_multiple_matches("ARGS");
        let arguments = ArgumentExtractor::extract_arguments(&args_nodes);

        // Get position information from the method call node
        let node = node_match.get_node();
        let start = node.start_pos();
        let end = node.end_pos();

        Some(ResourceMethodCallInfo {
            resource_var,
            method_name,
            arguments,
            method_call_line: start.line() + 1,
            start_position: (start.line() + 1, start.column(node) + 1),
            end_position: (end.line() + 1, end.column(node) + 1),
        })
    }

    /// Parse a simple resource method call (without assignment)
    fn parse_simple_resource_method_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Option<ResourceMethodCallInfo> {
        let env = node_match.get_env();

        // Extract resource variable name
        let resource_var = env.get_match("RESOURCE_VAR")?.text().to_string();

        // Extract method name
        let method_name = env.get_match("METHOD")?.text().to_string();

        // Extract arguments
        let args_nodes = env.get_multiple_matches("ARGS");
        let arguments = ArgumentExtractor::extract_arguments(&args_nodes);

        // Get position information from the method call node
        let node = node_match.get_node();
        let start = node.start_pos();
        let end = node.end_pos();

        Some(ResourceMethodCallInfo {
            resource_var,
            method_name,
            arguments,
            method_call_line: start.line() + 1,
            start_position: (start.line() + 1, start.column(node) + 1),
            end_position: (end.line() + 1, end.column(node) + 1),
        })
    }

    /// Create a single accurate SdkMethodCall using boto3 specification with waiter resolution
    fn create_synthetic_method_call_with_waiter_resolution(
        &self,
        method_call: &ResourceMethodCallInfo,
        constructor: &ResourceConstructorInfo,
        boto3_model: &Boto3ResourcesModel,
    ) -> Option<SdkMethodCall> {
        // Look up action mapping in boto3 model
        let action_mapping =
            boto3_model.get_action_mapping(&constructor.resource_type, &method_call.method_name)?;

        // Get resource definition for identifier mappings
        let resource_def = boto3_model.get_resource_definition(&constructor.resource_type)?;

        // Resolve the actual operation name using type-safe pattern matching
        let resolved_operation = match &action_mapping.operation {
            OperationType::Waiter { waiter_name } => {
                // Resolve actual operation via ServiceModelIndex
                if let Some(service_methods) = self.service_index.waiter_lookup.get(waiter_name) {
                    let service_methods_filtered = service_methods
                        .iter()
                        .filter(|x| x.service_name == constructor.service_name)
                        .collect::<Vec<_>>();
                    match service_methods_filtered.first() {
                        None => {
                            log::debug!(
                                "Service '{}' not found in ServiceModelIndex",
                                constructor.service_name
                            );
                            return None;
                        }
                        Some(service_method) => service_method.operation_name.to_case(Case::Snake),
                    }
                } else {
                    log::debug!(
                        "Waiter '{}' not found in service '{}' waiters",
                        waiter_name,
                        constructor.service_name
                    );
                    return None;
                }
            }
            OperationType::SdkOperation(op_name) | OperationType::Load(op_name) => {
                op_name.to_case(Case::Snake)
            }
        };

        // Build parameters list starting with identifier parameters
        let mut combined_parameters = Vec::new();

        // Inject identifier parameters from boto3 spec
        for param_mapping in &action_mapping.identifier_params {
            if let Some(param_name) = &param_mapping.name {
                // Find the identifier definition to get the value position
                if let Some(_identifier) = resource_def
                    .identifiers
                    .iter()
                    .find(|id| id.name == *param_name)
                {
                    // Get value from constructor args
                    // For now, we assume the first positional arg is the identifier value
                    if let Some(first_arg) = constructor.constructor_args.first() {
                        let value = match first_arg {
                            Parameter::Positional { value, .. } => value.clone(),
                            Parameter::Keyword { value, .. } => value.clone(),
                            Parameter::DictionarySplat { expression, .. } => {
                                ParameterValue::Unresolved(expression.clone())
                            }
                        };

                        // Use the target parameter name from boto3 spec
                        combined_parameters.push(Parameter::Keyword {
                            name: param_mapping.target.clone(),
                            value,
                            position: combined_parameters.len(),
                            type_annotation: None,
                        });
                    }
                }
            }
        }

        // Add method call arguments
        for (i, param) in method_call.arguments.iter().enumerate() {
            let adjusted_param = match param {
                Parameter::Keyword {
                    name,
                    value,
                    type_annotation,
                    ..
                } => Parameter::Keyword {
                    name: name.clone(),
                    value: value.clone(),
                    position: combined_parameters.len() + i,
                    type_annotation: type_annotation.clone(),
                },
                Parameter::Positional {
                    value,
                    type_annotation,
                    ..
                } => Parameter::Positional {
                    value: value.clone(),
                    position: combined_parameters.len() + i,
                    type_annotation: type_annotation.clone(),
                    struct_fields: None,
                },
                Parameter::DictionarySplat { expression, .. } => Parameter::DictionarySplat {
                    expression: expression.clone(),
                    position: combined_parameters.len() + i,
                },
            };
            combined_parameters.push(adjusted_param);
        }

        Some(SdkMethodCall {
            name: resolved_operation,
            possible_services: vec![constructor.service_name.clone()],
            metadata: Some(SdkMethodCallMetadata {
                parameters: combined_parameters,
                return_type: None,
                start_position: method_call.start_position,
                end_position: method_call.end_position,
                receiver: Some(method_call.resource_var.clone()),
            }),
        })
    }

    /// Find hasMany collection accesses and generate synthetic calls (Tier 2 approach)
    ///
    /// Detects patterns like: `collection = resource.collection_name`
    /// Generates synthetic SdkMethodCall for the collection's operation at the access point
    fn find_and_generate_collection_synthetics(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
        constructors: &[ResourceConstructorInfo],
    ) -> Vec<SdkMethodCall> {
        let root = ast.root();
        let mut synthetic_calls = Vec::new();

        // Pattern: $VAR = $RESOURCE_VAR.$ATTR_NAME (with optional assignment)
        // We'll use two patterns to catch both cases
        let patterns = vec![
            "$VAR = $RESOURCE_VAR.$ATTR_NAME", // With assignment
            "$RESOURCE_VAR.$ATTR_NAME",        // Without assignment (direct usage)
        ];

        for pattern in patterns {
            for node_match in root.find_all(pattern) {
                let env = node_match.get_env();

                // Extract resource variable name
                let resource_var = match env.get_match("RESOURCE_VAR") {
                    Some(node) => node.text().to_string(),
                    None => continue,
                };

                // Extract attribute name
                let attr_name = match env.get_match("ATTR_NAME") {
                    Some(node) => node.text().to_string(),
                    None => continue,
                };

                // Find the constructor for this resource variable
                let constructor = match constructors
                    .iter()
                    .find(|c| c.variable_name == resource_var)
                {
                    Some(c) => c,
                    None => continue,
                };

                // Get boto3 model for this service
                let boto3_model = match self.registry.get_model(&constructor.service_name) {
                    Some(model) => model,
                    None => continue,
                };

                // Check if this attribute matches a hasMany collection (in snake_case)
                if let Some(has_many_spec) =
                    boto3_model.get_has_many_spec(&constructor.resource_type, &attr_name)
                {
                    // Generate synthetic call for the collection's operation
                    let node = node_match.get_node();
                    let start = node.start_pos();
                    let end = node.end_pos();

                    if let Some(synthetic_call) = self.generate_synthetic_for_collection(
                        constructor,
                        has_many_spec,
                        (start.line() + 1, start.column(node) + 1),
                        (end.line() + 1, end.column(node) + 1),
                    ) {
                        synthetic_calls.push(synthetic_call);
                    }
                }
            }
        }

        synthetic_calls
    }

    /// Generate a synthetic SdkMethodCall for a hasMany collection access
    fn generate_synthetic_for_collection(
        &self,
        constructor: &ResourceConstructorInfo,
        has_many_spec: &HasManySpec,
        start_position: (usize, usize),
        end_position: (usize, usize),
    ) -> Option<SdkMethodCall> {
        let mut parameters = Vec::new();

        // Inject identifier parameters from parent resource constructor
        for param_mapping in &has_many_spec.identifier_params {
            if param_mapping.name.is_some() {
                // Match the identifier from constructor args
                // For simplicity, we use the first constructor arg for the first identifier
                if let Some(first_arg) = constructor.constructor_args.first() {
                    let value = match first_arg {
                        Parameter::Positional { value, .. } => value.clone(),
                        Parameter::Keyword { value, .. } => value.clone(),
                        Parameter::DictionarySplat { expression, .. } => {
                            ParameterValue::Unresolved(expression.clone())
                        }
                    };

                    parameters.push(Parameter::Keyword {
                        name: param_mapping.target.clone(),
                        value,
                        position: parameters.len(),
                        type_annotation: None,
                    });
                }
            }
        }

        Some(SdkMethodCall {
            name: has_many_spec.operation.to_case(Case::Snake),
            possible_services: vec![constructor.service_name.clone()],
            metadata: Some(SdkMethodCallMetadata {
                parameters,
                return_type: None,
                start_position,
                end_position,
                receiver: Some(constructor.variable_name.clone()), // Use actual variable name from constructor
            }),
        })
    }

    /// New Tier 3: Find unmatched utility methods and collection accesses (conservative fallback)
    ///
    /// Searches for method calls and attribute accesses that match utility/collection patterns
    /// but were NOT matched in Tiers 1/2 (unknown receivers). Generates synthetics with
    /// all-synthetic parameters since we don't know the receiver.
    fn find_unmatched_utility_and_collection_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
        matched_positions: &HashSet<MatchedPosition>,
    ) -> Vec<SdkMethodCall> {
        let mut tier3_calls = Vec::new();

        // Search for utility method calls across all services
        tier3_calls.extend(self.find_unmatched_utility_method_calls(ast, matched_positions));

        // Search for collection accesses across all services
        tier3_calls.extend(self.find_unmatched_collection_accesses(ast, matched_positions));

        tier3_calls
    }

    /// Find utility method calls with unknown receivers (Tier 3)
    fn find_unmatched_utility_method_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
        matched_positions: &HashSet<MatchedPosition>,
    ) -> Vec<SdkMethodCall> {
        let root = ast.root();
        let mut calls = Vec::new();

        // Pattern for method calls
        let patterns = vec!["$RESULT = $VAR.$METHOD($$$ARGS)", "$VAR.$METHOD($$$ARGS)"];

        for pattern in patterns {
            for node_match in root.find_all(pattern) {
                let env = node_match.get_env();

                // Extract receiver variable name
                let receiver_var = match env.get_match("VAR") {
                    Some(node) => node.text().to_string(),
                    None => continue,
                };

                // Extract method name
                let method_name = match env.get_match("METHOD") {
                    Some(node) => node.text().to_string(),
                    None => continue,
                };

                // Get position
                let node = node_match.get_node();
                let start = node.start_pos();
                let position = MatchedPosition {
                    line: start.line() + 1,
                    column: start.column(node) + 1,
                };

                // Skip if already matched in Tier 1/2
                if matched_positions.contains(&position) {
                    continue;
                }

                // Extract arguments
                let args_nodes = env.get_multiple_matches("ARGS");
                let arguments = ArgumentExtractor::extract_arguments(&args_nodes);

                // Search for this method name across all services
                for (service_name, boto3_model) in self.registry.models() {
                    // Check client utility methods with parameter count filtering
                    if let Some(client_method) = boto3_model.get_client_utility_method(&method_name)
                    {
                        // Generate synthetic for each operation
                        for operation in &client_method.operations {
                            // Filter: Skip if call site has fewer args than required
                            // Client methods show all parameters at call site (unlike resource methods
                            // where constructor parameters are hidden)
                            if arguments.len() < operation.required_params.len() {
                                continue; // Not enough arguments to satisfy this operation
                            }

                            calls.push(self.generate_tier3_utility_synthetic(
                                service_name,
                                &operation.operation,
                                &arguments,
                                &operation.required_params,
                                (start.line() + 1, start.column(node) + 1),
                                (node.end_pos().line() + 1, node.end_pos().column(node) + 1),
                                &receiver_var, // Use actual receiver from code
                            ));
                        }
                    }

                    // Check resource utility methods across all resource types
                    for resource_methods in boto3_model.get_all_resource_utility_methods().values()
                    {
                        if let Some(resource_method) = resource_methods.methods.get(&method_name) {
                            // Generate synthetic for each operation
                            for operation in &resource_method.operations {
                                calls.push(self.generate_tier3_utility_synthetic(
                                    service_name,
                                    &operation.operation,
                                    &arguments,
                                    &operation.required_params,
                                    (start.line() + 1, start.column(node) + 1),
                                    (node.end_pos().line() + 1, node.end_pos().column(node) + 1),
                                    &receiver_var, // Use actual receiver from code
                                ));
                            }
                        }
                    }
                }
            }
        }

        calls
    }

    /// Find collection accesses with unknown receivers (Tier 3)
    fn find_unmatched_collection_accesses(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
        matched_positions: &HashSet<MatchedPosition>,
    ) -> Vec<SdkMethodCall> {
        let root = ast.root();
        let mut calls = Vec::new();

        // Patterns for attribute access (including chained method calls)
        let patterns = vec![
            "$VAR = $RESOURCE_VAR.$ATTR_NAME", // Simple: var = resource.collection
            "$RESOURCE_VAR.$ATTR_NAME",        // Direct: resource.collection
            "$VAR = $RESOURCE_VAR.$ATTR_NAME.$$$REST", // Chained: var = resource.collection.method(...)
            "$RESOURCE_VAR.$ATTR_NAME.$$$REST", // Direct chained: resource.collection.method(...)
        ];

        for pattern in patterns {
            for node_match in root.find_all(pattern) {
                let env = node_match.get_env();

                // Extract receiver variable name
                let receiver_var = match env.get_match("RESOURCE_VAR") {
                    Some(node) => node.text().to_string(),
                    None => continue,
                };

                // Extract attribute name
                let attr_name = match env.get_match("ATTR_NAME") {
                    Some(node) => node.text().to_string(),
                    None => continue,
                };

                // Get position
                let node = node_match.get_node();
                let start = node.start_pos();
                let position = MatchedPosition {
                    line: start.line() + 1,
                    column: start.column(node) + 1,
                };

                // Skip if already matched in Tier 1/2
                if matched_positions.contains(&position) {
                    continue;
                }

                // Search for this collection name across all services
                for (service_name, boto3_model) in self.registry.models() {
                    // Check all resource types for hasMany collections (resource-level)
                    for resource_def in boto3_model.get_all_resource_definitions().values() {
                        if let Some(has_many_spec) = resource_def.has_many.get(&attr_name) {
                            // Generate synthetic with all-synthetic parameters
                            calls.push(SdkMethodCall {
                                name: has_many_spec.operation.to_case(Case::Snake),
                                possible_services: vec![service_name.clone()],
                                metadata: Some(SdkMethodCallMetadata {
                                    parameters: self.generate_synthetic_parameters(
                                        &has_many_spec.identifier_params,
                                    ),
                                    return_type: None,
                                    start_position: (start.line() + 1, start.column(node) + 1),
                                    end_position: (
                                        node.end_pos().line() + 1,
                                        node.end_pos().column(node) + 1,
                                    ),
                                    receiver: Some(receiver_var.clone()), // Use actual receiver from code
                                }),
                            });
                        }
                    }

                    // Check service-level hasMany collections
                    if let Some(service_has_many_spec) = boto3_model
                        .get_service_has_many_collections()
                        .get(&attr_name)
                    {
                        // Generate synthetic with all-synthetic parameters (service-level collections typically have no identifier params)
                        calls.push(SdkMethodCall {
                            name: service_has_many_spec.operation.to_case(Case::Snake),
                            possible_services: vec![service_name.clone()],
                            metadata: Some(SdkMethodCallMetadata {
                                parameters: self.generate_synthetic_parameters(
                                    &service_has_many_spec.identifier_params,
                                ),
                                return_type: None,
                                start_position: (start.line() + 1, start.column(node) + 1),
                                end_position: (
                                    node.end_pos().line() + 1,
                                    node.end_pos().column(node) + 1,
                                ),
                                receiver: Some(receiver_var.clone()), // Use actual receiver from code
                            }),
                        });
                    }
                }
            }
        }

        calls
    }

    /// Generate synthetic SdkMethodCall for Tier 3 utility method (all synthetic params)
    #[allow(clippy::too_many_arguments)]
    fn generate_tier3_utility_synthetic(
        &self,
        service_name: &str,
        operation: &str,
        arguments: &[Parameter],
        required_params: &[String],
        start_position: (usize, usize),
        end_position: (usize, usize),
        receiver_marker: &str,
    ) -> SdkMethodCall {
        let mut parameters = Vec::new();

        // Add user-provided arguments (keep actual values when available)
        for arg in arguments {
            parameters.push(arg.clone());
        }

        // Add synthetic values for missing required parameters
        for required_param in required_params {
            let param_exists = parameters
                .iter()
                .any(|p| matches!(p, Parameter::Keyword { name, .. } if name == required_param));

            if !param_exists {
                parameters.push(Parameter::Keyword {
                    name: required_param.clone(),
                    value: ParameterValue::Unresolved(format!(
                        "synthetic_{}",
                        required_param.to_case(Case::Snake)
                    )),
                    position: parameters.len(),
                    type_annotation: None,
                });
            }
        }

        SdkMethodCall {
            name: operation.to_case(Case::Snake),
            possible_services: vec![service_name.to_string()],
            metadata: Some(SdkMethodCallMetadata {
                parameters,
                return_type: None,
                start_position,
                end_position,
                receiver: Some(receiver_marker.to_string()),
            }),
        }
    }

    /// Generate all-synthetic parameters for collection access (Tier 3)
    fn generate_synthetic_parameters(
        &self,
        param_mappings: &[crate::extraction::python::boto3_resources_model::ParamMapping],
    ) -> Vec<Parameter> {
        param_mappings
            .iter()
            .enumerate()
            .map(|(i, mapping)| Parameter::Keyword {
                name: mapping.target.clone(),
                value: ParameterValue::Unresolved(format!(
                    "synthetic_{}",
                    mapping.target.to_case(Case::Snake)
                )),
                position: i,
                type_annotation: None,
            })
            .collect()
    }
}
