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
use crate::extraction::python::node_kinds;
use crate::extraction::sdk_model::ServiceDiscovery;
use crate::extraction::{
    AstWithSourceFile, Parameter, ParameterValue, SdkMethodCall, SdkMethodCallMetadata,
};
use crate::{Location, ServiceModelIndex};
use ast_grep_language::Python;
use convert_case::{Case, Casing};
use std::collections::{HashMap, HashSet};
use std::path::Path;

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
    expr: String,
    location: Location,
}

impl ResourceMethodCallInfo {
    fn start_line(&self) -> usize {
        self.location.start_line()
    }
}

/// The leaf resource a sub-resource chain resolves to, with its identifier values.
///
/// `identifiers` maps each identifier name on `resource_type` (e.g. `BucketName`, `Key`)
/// to its resolved value. Keyed by name — not position — so values are carried and
/// injected by identifier name throughout, matching how boto3 specs reference them.
#[derive(Debug, Clone)]
struct ResolvedResource {
    service_name: String,
    resource_type: String,
    identifiers: HashMap<String, ParameterValue>,
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
        ast: &AstWithSourceFile<Python>,
    ) -> Vec<SdkMethodCall> {
        // Step 1: Find all resource constructors using service-agnostic matching
        let mut constructors = self.find_resource_constructors(ast, &self.registry);

        // Step 2: Find all method calls on resource objects
        let mut method_calls = self.find_resource_method_calls(ast);

        // Step 2b: Resolve nested sub-resource action calls, including those on a variable
        // bound to a nested chain (e.g. `s3.Bucket("b").Object("k").put(...)` and
        // `obj = s3.Bucket("b").Object("k"); obj.put(...)`). These synthesize a
        // constructor + method-call pair sharing a unique variable name so the matching
        // step below joins them (injecting the accumulated identifiers).
        let (chained_constructors, chained_method_calls) =
            self.find_chained_subresource_calls(ast, &self.registry);
        constructors.extend(chained_constructors);
        method_calls.extend(chained_method_calls);

        // Step 3: Match each method call against its resource's boto3 spec. Unmatched
        // methods on a known resource simply produce nothing (an unknown method is not
        // evidence for any particular operation).
        let mut all_calls =
            self.match_resource_method_calls(&constructors, &method_calls, self.registry.models());

        // Step 4: Find and generate synthetic calls for hasMany collections
        let collection_synthetics =
            self.find_and_generate_collection_synthetics(ast, &constructors);
        all_calls.extend(collection_synthetics);

        // Step 5: Collect matched positions for Tier 3 deduplication
        let mut matched_locations = HashSet::new();
        for call in &all_calls {
            if let Some(metadata) = &call.metadata {
                matched_locations.insert(metadata.location.clone());
            }
        }

        // Step 6: Service-agnostic fallback for unknown receivers
        let tier3_calls = self.find_unmatched_utility_and_collection_calls(ast, &matched_locations);
        all_calls.extend(tier3_calls);

        all_calls
    }

    /// Match resource method calls against their resource's boto3 spec, returning the
    /// resolved [`SdkMethodCall`]s. A method call resolves either to a direct action or to
    /// the expansion of a utility method (e.g. `upload_file`). Method calls that match
    /// neither produce nothing: an unknown method on a known resource is not evidence for
    /// any particular operation.
    fn match_resource_method_calls(
        &self,
        constructors: &[ResourceConstructorInfo],
        method_calls: &[ResourceMethodCallInfo],
        boto3_models: &HashMap<String, Boto3ResourcesModel>,
    ) -> Vec<SdkMethodCall> {
        let mut matched_calls = Vec::new();

        for constructor in constructors {
            // Find all method calls for this resource
            let resource_methods = method_calls
                .iter()
                .filter(|mc| mc.resource_var == constructor.variable_name);

            // Try to match each method call
            let boto3_model = match boto3_models.get(&constructor.service_name) {
                Some(model) => model,
                None => continue,
            };

            for method_call in resource_methods {
                if let Some(call) = self.try_match_method(method_call, constructor, boto3_model) {
                    matched_calls.push(call);
                    continue;
                }

                let utility_calls =
                    self.try_expand_resource_utility_method(method_call, constructor, boto3_model);
                matched_calls.extend(utility_calls);
            }
        }

        matched_calls
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

            let metadata =
                SdkMethodCallMetadata::new(method_call.expr.clone(), method_call.location.clone())
                    .with_parameters(parameters)
                    .with_receiver(method_call.resource_var.clone());

            expanded_calls.push(SdkMethodCall {
                name: operation.operation.to_case(Case::Snake),
                possible_services: vec![constructor.service_name.clone()],
                metadata: Some(metadata),
            });
        }

        expanded_calls
    }

    /// Find all resource constructor calls in the AST using service-agnostic matching
    fn find_resource_constructors(
        &self,
        ast: &AstWithSourceFile<Python>,
        registry: &Boto3ResourcesRegistry,
    ) -> Vec<ResourceConstructorInfo> {
        let root = ast.ast.root();
        let mut constructors = Vec::new();

        // Service-agnostic pattern: $VAR = $ANY.$RESOURCE_TYPE($$$ARGS)
        // This matches ANY object calling a method, regardless of how the service was instantiated
        let constructor_pattern = "$VAR = $ANY.$RESOURCE_TYPE($$$ARGS)";

        for node_match in root.find_all(constructor_pattern) {
            let match_env = node_match.get_env();

            // Extract variable name
            let variable_name = match match_env.get_match("VAR") {
                Some(node) => node.text().to_string(),
                None => continue,
            };

            // Extract resource type (e.g., "Table", "Bucket")
            let resource_type = match match_env.get_match("RESOURCE_TYPE") {
                Some(node) => node.text().to_string(),
                None => continue,
            };

            // Look up which services provide this resource type
            let possible_services = registry.find_services_for_resource(&resource_type);

            if possible_services.is_empty() {
                continue; // Not a known resource type
            }

            // Extract arguments
            let args_nodes = match_env.get_multiple_matches("ARGS");
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

    /// Find nested sub-resource action calls, including those on a variable bound to a
    /// nested sub-resource chain.
    ///
    /// Handles:
    /// - Inline action chains of any depth: `s3.Bucket("b").put_object(...)`,
    ///   `s3.Bucket("b").Object("k").put(...)`.
    /// - Nested assigned chains followed by an action:
    ///   `obj = s3.Bucket("b").Object("k")` then `obj.put(...)`.
    ///
    /// The single-level *assigned* form (`bucket = s3.Bucket("b"); bucket.put_object(...)`)
    /// is already handled by [`find_resource_constructors`] +
    /// [`find_resource_method_calls`]. To avoid double-counting it, the assignment pass
    /// here gates on chain depth ≥ 2; the inline action pass has no such overlap (an inline
    /// constructor is never registered by the existing path) so it accepts any depth.
    ///
    /// Each resolved action becomes a synthetic constructor + method-call pair (sharing a
    /// unique synthetic variable name) so the regular matching path joins them and injects
    /// the accumulated identifiers. Any chain whose links do not all resolve is dropped
    /// entirely (logged at debug), rather than emitting partial or speculative operations.
    fn find_chained_subresource_calls(
        &self,
        ast: &AstWithSourceFile<Python>,
        registry: &Boto3ResourcesRegistry,
    ) -> (Vec<ResourceConstructorInfo>, Vec<ResourceMethodCallInfo>) {
        let root = ast.ast.root();
        let mut constructors = Vec::new();
        let mut method_calls = Vec::new();

        // First pass: resolve assignments whose RHS is a nested sub-resource chain, e.g.
        // `obj = s3.Bucket("b").Object("k")`. Maps the assigned variable name to the
        // resolved leaf resource so later `obj.method(...)` calls can be matched.
        let assigned_resources = self.resolve_assigned_resource_chains(ast, registry);

        // Second pass: every call `<receiver>.method(args)`. The receiver is either an
        // inline nested chain or a variable bound to one (from the first pass).
        for node_match in root.find_all("$RECEIVER.$METHOD($$$ARGS)") {
            let env = node_match.get_env();
            let call_node = node_match.get_node();

            let method_name = match env.get_match("METHOD") {
                Some(node) => node.text().to_string(),
                None => continue,
            };
            let receiver_node = match env.get_match("RECEIVER") {
                Some(node) => node,
                None => continue,
            };

            let args_nodes = env.get_multiple_matches("ARGS");
            let arguments = ArgumentExtractor::extract_arguments(&args_nodes);
            let location = Location::from_node(ast.source_file.path.clone(), call_node);
            let start = call_node.start_pos();

            // Resolve the receiver to a leaf resource: either a variable bound to a nested
            // chain, or an inline nested chain. `resolve_chain` yields one per service.
            let receiver_text = receiver_node.text().to_string();
            let resolved = match assigned_resources.get(&receiver_text) {
                Some(by_var) => by_var.clone(),
                None => self.resolve_chain(receiver_node, registry),
            };

            for resource in resolved {
                // `find_all` also matches the intermediate calls of a chain: for
                // `s3.Bucket("b").Object("k").put(...)` it yields the `.Object("k")` call
                // too, whose method is itself a sub-resource navigation rather than an
                // action. Skip those — the outer match that includes the next link already
                // covers them, and a navigation never resolves to an action downstream.
                if let Some(model) = registry.get_model(&resource.service_name) {
                    if model
                        .get_sub_resource(&resource.resource_type, &method_name)
                        .is_some()
                    {
                        continue;
                    }
                }

                let synthetic_var = format!(
                    "__chained_{}_{}_{}_{}",
                    resource.service_name,
                    resource.resource_type,
                    start.line() + 1,
                    start.column(call_node) + 1
                );

                // Carry each resolved identifier as a named keyword arg so the Tier 1
                // matcher injects it by name into the operation's required parameters.
                let constructor_args = resource
                    .identifiers
                    .iter()
                    .enumerate()
                    .map(|(position, (name, value))| Parameter::Keyword {
                        name: name.clone(),
                        value: value.clone(),
                        position,
                        type_annotation: None,
                    })
                    .collect();

                constructors.push(ResourceConstructorInfo {
                    variable_name: synthetic_var.clone(),
                    resource_type: resource.resource_type.clone(),
                    service_name: resource.service_name.clone(),
                    constructor_args,
                    start_position: (start.line() + 1, start.column(call_node) + 1),
                    end_position: (start.line() + 1, start.column(call_node) + 1),
                });

                method_calls.push(ResourceMethodCallInfo {
                    resource_var: synthetic_var,
                    method_name: method_name.clone(),
                    arguments: arguments.clone(),
                    expr: node_match.text().to_string(),
                    location: location.clone(),
                });
            }
        }

        (constructors, method_calls)
    }

    /// Resolve assignments of the form `var = <nested sub-resource chain>` (e.g.
    /// `obj = s3.Bucket("b").Object("k")`), returning a map from the assigned variable
    /// name to the resolved leaf resource(s). Assignments whose RHS does not resolve to a
    /// nested resource chain are omitted.
    fn resolve_assigned_resource_chains(
        &self,
        ast: &AstWithSourceFile<Python>,
        registry: &Boto3ResourcesRegistry,
    ) -> HashMap<String, Vec<ResolvedResource>> {
        let root = ast.ast.root();
        let mut assigned = HashMap::new();

        for node_match in root.find_all("$VAR = $RHS") {
            let env = node_match.get_env();
            let var_name = match env.get_match("VAR") {
                Some(node) => node.text().to_string(),
                None => continue,
            };
            let rhs_node = match env.get_match("RHS") {
                Some(node) => node,
                None => continue,
            };

            // Only handle NESTED assignments here (depth ≥ 2, e.g.
            // `obj = s3.Bucket("b").Object("k")`). Single-level assignments
            // (`bucket = s3.Bucket("b")`) are owned by the existing constructor +
            // method-call path; resolving them here too would double-count.
            let depth = Self::collect_chain_links(rhs_node).map_or(0, |links| links.len());
            if depth < 2 {
                continue;
            }

            let resolved = self.resolve_chain(rhs_node, registry);
            if !resolved.is_empty() {
                assigned.insert(var_name, resolved);
            }
        }

        assigned
    }

    /// Collect a method/navigation chain as `(accessor, args)` links in base-first order.
    ///
    /// For `s3.Bucket("b").Object("k")` this yields `[("Bucket", ["b"]), ("Object", ["k"])]`.
    /// Walks the receiver inward (each link is a `call` whose function is an `attribute`),
    /// stopping at the base object (e.g. the `s3` identifier). Returns `None` if the node
    /// is malformed in a way that should abort resolution.
    fn collect_chain_links(
        node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Option<Vec<(String, Vec<Parameter>)>> {
        let mut links = Vec::new(); // (accessor, args), outermost first
        let mut current = node.clone();
        loop {
            if current.kind() != node_kinds::CALL {
                break;
            }
            let function = current.field("function")?;
            if function.kind() != node_kinds::ATTRIBUTE {
                // Base is not an attribute access (e.g. a bare name); the outermost
                // construct must be `<receiver>.Accessor(args)` to be a chain.
                break;
            }
            let accessor = function.field("attribute")?.text().to_string();
            let args = match current.field("arguments") {
                Some(arg_list) => {
                    // `argument_list` children include punctuation tokens (`(`, `)`, `,`);
                    // keep only named nodes so the extractor sees just the arguments.
                    let arg_nodes: Vec<_> = arg_list
                        .children()
                        .filter(ast_grep_core::Node::is_named)
                        .collect();
                    ArgumentExtractor::extract_arguments(&arg_nodes)
                }
                None => Vec::new(),
            };
            links.push((accessor, args));
            current = function.field("object")?;
        }

        // Collected outermost-first; reverse to base-first.
        links.reverse();
        Some(links)
    }

    /// Resolve an expression node to the leaf resource(s) of a boto3 sub-resource chain.
    ///
    /// Walks the chain outward from its base. The base must be a known resource
    /// constructor call (e.g. `s3.Bucket("b")`); each subsequent link must be a known
    /// sub-resource navigation (e.g. `.Object("k")`). Identifier values are accumulated
    /// across links in the order of the leaf resource's identifiers. Returns one
    /// [`ResolvedResource`] per service that provides the base resource type.
    ///
    /// Resolves chains of any depth, including a bare base (`s3.Bucket("b")`). Callers
    /// that overlap with the existing single-level path (e.g. assignment resolution)
    /// gate on chain depth themselves.
    ///
    /// Returns an empty vector if `node` is not a resource chain, or if any link fails to
    /// resolve (unknown accessor, arg-count mismatch, missing model) — chains are resolved
    /// fully or not at all.
    fn resolve_chain(
        &self,
        node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
        registry: &Boto3ResourcesRegistry,
    ) -> Vec<ResolvedResource> {
        let links = match Self::collect_chain_links(node) {
            Some(links) => links,
            None => return Vec::new(),
        };

        let (base_accessor, base_args) = match links.first() {
            Some(first) => first,
            None => return Vec::new(),
        };

        // The base accessor must be a known resource type provided by some service(s).
        let services = registry.find_services_for_resource(base_accessor);
        if services.is_empty() {
            return Vec::new();
        }

        let mut results = Vec::new();
        for service_name in services {
            if let Some(resource) =
                self.resolve_chain_for_service(&service_name, &links, base_args, registry)
            {
                results.push(resource);
            }
        }
        results
    }

    /// Resolve a chain's links against a single service's model, accumulating identifier
    /// values. Returns `None` if any link is unresolvable for this service.
    fn resolve_chain_for_service(
        &self,
        service_name: &str,
        links: &[(String, Vec<Parameter>)],
        base_args: &[Parameter],
        registry: &Boto3ResourcesRegistry,
    ) -> Option<ResolvedResource> {
        let model = registry.get_model(service_name)?;

        // Base resource: assign the constructor args to its identifiers by name. Keyword
        // args match the identifier's snake_case name; positional args fill the remaining
        // identifiers in declared order. Yields a name -> value map for the base resource.
        let (base_accessor, _) = links.first()?;
        let base_spec = model.get_constructor_spec(base_accessor)?;
        let base_identifier_names: Vec<&str> = model
            .get_resource_definition(&base_spec.resource_type)?
            .identifiers
            .iter()
            .map(|id| id.name.as_str())
            .collect();

        let mut current_type = base_spec.resource_type.clone();
        let mut identifiers = Self::assign_args_to_identifiers(base_args, &base_identifier_names)?;

        // Walk the remaining links as sub-resource navigations.
        for (accessor, nav_args) in &links[1..] {
            let sub = model.get_sub_resource(&current_type, accessor)?;

            // The navigation's own arguments supply the `input`-sourced identifiers, in
            // their declared order, keyed by their target name. This validates the count,
            // so a navigation given the wrong number of args bails (like the base does).
            let input_names: Vec<&str> = sub
                .identifiers
                .iter()
                .filter(|id| id.source == "input")
                .map(|id| id.target.as_str())
                .collect();
            let mut input_values = Self::assign_args_to_identifiers(nav_args, &input_names)?;

            // Build the child's identifier map: each value comes from the parent (source
            // "identifier", looked up by the named parent identifier) or from this
            // navigation (source "input"), keyed by the child's target identifier name.
            let mut next_identifiers = HashMap::new();
            for ident in &sub.identifiers {
                let value = match ident.source.as_str() {
                    "identifier" => identifiers.get(ident.name.as_deref()?)?.clone(),
                    "input" => input_values.remove(&ident.target)?,
                    other => {
                        log::debug!("Unsupported sub-resource identifier source '{other}'");
                        return None;
                    }
                };
                next_identifiers.insert(ident.target.clone(), value);
            }

            current_type = sub.resource_type.clone();
            identifiers = next_identifiers;
        }

        Some(ResolvedResource {
            service_name: service_name.to_string(),
            resource_type: current_type,
            identifiers,
        })
    }

    /// Assign a call's arguments to the named `identifier_names`, returning a
    /// `name -> value` map.
    ///
    /// boto3 resource constructors and sub-resource navigations accept their identifiers
    /// positionally or by keyword, where the keyword is the snake_case of the identifier
    /// name (e.g. `BucketName` -> `bucket_name`). Keyword args are matched to the identifier
    /// whose snake_case name they equal; positional args fill the remaining identifiers in
    /// declared order. Returns `None` unless every identifier is supplied exactly once and
    /// no extra args remain — so a call with the wrong number or names of args bails rather
    /// than mismapping.
    fn assign_args_to_identifiers(
        args: &[Parameter],
        identifier_names: &[&str],
    ) -> Option<HashMap<String, ParameterValue>> {
        // A `**kwargs` splat hides identifier values we cannot map; bail.
        if args
            .iter()
            .any(|a| matches!(a, Parameter::DictionarySplat { .. }))
        {
            return None;
        }

        // Wrong arg count can never line up one-to-one with the identifiers.
        if args.len() != identifier_names.len() {
            return None;
        }

        // Index keyword args by name; collect positional values in order.
        let mut keyword: HashMap<&str, &ParameterValue> = HashMap::new();
        let mut positional: Vec<&ParameterValue> = Vec::new();
        for arg in args {
            match arg {
                Parameter::Keyword { name, value, .. } => {
                    // A duplicate keyword would be invalid Python; treat as unresolvable.
                    if keyword.insert(name.as_str(), value).is_some() {
                        return None;
                    }
                }
                Parameter::Positional { value, .. } => positional.push(value),
                Parameter::DictionarySplat { .. } => unreachable!("splats rejected above"),
            }
        }

        // Assign exactly like Python: positional args fill the FIRST identifiers in
        // declared order; keyword args then fill the REMAINING identifiers by name. A
        // keyword that targets an already positionally-filled identifier is a collision
        // ("got multiple values for argument") — invalid Python, so we bail.
        let mut assigned = HashMap::with_capacity(identifier_names.len());
        let mut identifiers = identifier_names.iter();

        for value in &positional {
            // `args.len() == identifier_names.len()` guarantees a slot exists here.
            let name = identifiers.next()?;
            assigned.insert((*name).to_string(), (*value).clone());
        }

        for name in identifiers {
            let kwarg_name =
                ServiceDiscovery::operation_to_method_name(name, crate::Language::Python);
            let value = keyword.remove(kwarg_name.as_str())?;
            assigned.insert((*name).to_string(), value.clone());
        }

        // Any keyword left over either named an identifier already filled positionally
        // (collision) or named no identifier at all — both mean the call doesn't line up.
        if !keyword.is_empty() {
            return None;
        }

        Some(assigned)
    }

    /// Find all method calls on potential resource objects
    fn find_resource_method_calls(
        &self,
        ast: &AstWithSourceFile<Python>,
    ) -> Vec<ResourceMethodCallInfo> {
        let root = ast.ast.root();
        let mut method_calls = Vec::new();

        let method_call_pattern = "$RESULT = $RESOURCE_VAR.$METHOD($$$ARGS)";

        for node_match in root.find_all(method_call_pattern) {
            if let Some(method_call_info) =
                self.parse_resource_method_call(&node_match, &ast.source_file.path)
            {
                method_calls.push(method_call_info);
            }
        }

        // Also handle calls without assignment
        let simple_method_pattern = "$RESOURCE_VAR.$METHOD($$$ARGS)";

        for node_match in root.find_all(simple_method_pattern) {
            if let Some(method_call_info) =
                self.parse_simple_resource_method_call(&node_match, &ast.source_file.path)
            {
                method_calls.push(method_call_info);
            }
        }

        // Deduplicate method calls by (resource_var, method_name, line_number)
        method_calls.sort_by(|a, b| {
            a.resource_var
                .cmp(&b.resource_var)
                .then(a.method_name.cmp(&b.method_name))
                .then(a.start_line().cmp(&b.start_line()))
        });
        method_calls.dedup_by(|a, b| {
            a.resource_var == b.resource_var
                && a.method_name == b.method_name
                && a.start_line() == b.start_line()
        });

        method_calls
    }

    /// Parse a resource method call (with assignment)
    fn parse_resource_method_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
        file_path: &Path,
    ) -> Option<ResourceMethodCallInfo> {
        let env = node_match.get_env();

        // Extract resource variable name
        let resource_var = env.get_match("RESOURCE_VAR")?.text().to_string();

        // Extract method name
        let method_name = env.get_match("METHOD")?.text().to_string();

        // Extract arguments
        let args_nodes = env.get_multiple_matches("ARGS");
        let arguments = ArgumentExtractor::extract_arguments(&args_nodes);

        Some(ResourceMethodCallInfo {
            resource_var,
            method_name,
            arguments,
            expr: node_match.text().to_string(),
            location: Location::from_node(file_path.to_path_buf(), node_match.get_node()),
        })
    }

    /// Parse a simple resource method call (without assignment)
    fn parse_simple_resource_method_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
        file_path: &Path,
    ) -> Option<ResourceMethodCallInfo> {
        let env = node_match.get_env();

        // Extract resource variable name
        let resource_var = env.get_match("RESOURCE_VAR")?.text().to_string();

        // Extract method name
        let method_name = env.get_match("METHOD")?.text().to_string();

        // Extract arguments
        let args_nodes = env.get_multiple_matches("ARGS");
        let arguments = ArgumentExtractor::extract_arguments(&args_nodes);

        Some(ResourceMethodCallInfo {
            resource_var,
            method_name,
            arguments,
            expr: node_match.text().to_string(),
            location: Location::from_node(file_path.to_path_buf(), node_match.get_node()),
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

        // Inject identifier parameters from the boto3 spec. Each action `identifier_param`
        // maps a resource identifier (named by `param_mapping.name`, e.g. `BucketName`) to
        // the operation's target parameter (`param_mapping.target`, e.g. `Bucket`). Pull
        // the value from the constructor arg with the *matching identifier name* — not the
        // first arg — so a multi-identifier resource (e.g. `Object` -> `Bucket` + `Key`)
        // gives each target its own value rather than repeating the first.
        for param_mapping in &action_mapping.identifier_params {
            let Some(param_name) = &param_mapping.name else {
                continue;
            };
            if let Some(value) =
                Self::constructor_identifier_value(constructor, resource_def, param_name)
            {
                // Use the target parameter name from the boto3 spec.
                combined_parameters.push(Parameter::Keyword {
                    name: param_mapping.target.clone(),
                    value,
                    position: combined_parameters.len(),
                    type_annotation: None,
                });
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

        let metadata =
            SdkMethodCallMetadata::new(method_call.expr.clone(), method_call.location.clone())
                .with_parameters(combined_parameters)
                .with_receiver(method_call.resource_var.clone());

        Some(SdkMethodCall {
            name: resolved_operation,
            possible_services: vec![constructor.service_name.clone()],
            metadata: Some(metadata),
        })
    }

    /// Resolve the value of a resource identifier (e.g. `BucketName`) from a constructor's
    /// arguments.
    ///
    /// The chain resolver labels each constructor arg with its identifier name, so the
    /// arg whose keyword name matches is preferred — this is what lets a multi-identifier
    /// resource map each identifier to its own value. For example, for
    /// `s3.Bucket("b").Object("k").put(...)` the synthesized `Object` constructor carries
    /// `[Bucket="b", Key="k"]`, so `BucketName` resolves to `"b"` and `Key` to `"k"`.
    ///
    /// The fallback (positional arg at the identifier's declared index) covers the
    /// single-level path, where constructor args come straight from the user's call and
    /// carry no identifier label — e.g. `bucket = s3.Bucket("my-bucket")` produces an
    /// unnamed `Positional("my-bucket")`, which maps to `Name` at index 0.
    fn constructor_identifier_value(
        constructor: &ResourceConstructorInfo,
        resource_def: &crate::extraction::python::boto3_resources_model::ResourceDefinition,
        identifier_name: &str,
    ) -> Option<ParameterValue> {
        // Prefer the constructor arg explicitly labelled with this identifier's name.
        let by_name = constructor
            .constructor_args
            .iter()
            .find_map(|arg| match arg {
                Parameter::Keyword { name, .. } if name == identifier_name => Some(arg.value()),
                _ => None,
            });
        if by_name.is_some() {
            return by_name;
        }

        // Fallback: positional arg at the identifier's declared index.
        let index = resource_def
            .identifiers
            .iter()
            .position(|id| id.name == identifier_name)?;
        constructor
            .constructor_args
            .get(index)
            .map(Parameter::value)
    }

    /// Find hasMany collection accesses and generate synthetic calls (Tier 2 approach)
    ///
    /// Detects patterns like: `collection = resource.collection_name`
    /// Generates synthetic SdkMethodCall for the collection's operation at the access point
    fn find_and_generate_collection_synthetics(
        &self,
        ast: &AstWithSourceFile<Python>,
        constructors: &[ResourceConstructorInfo],
    ) -> Vec<SdkMethodCall> {
        let root = ast.ast.root();
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
                if let Some(synthetic_call) = boto3_model
                    .get_has_many_spec(&constructor.resource_type, &attr_name)
                    .and_then(|has_many_spec| {
                        self.generate_synthetic_for_collection(
                            constructor,
                            has_many_spec,
                            node_match.text().to_string(),
                            Location::from_node(
                                ast.source_file.path.clone(),
                                node_match.get_node(),
                            ),
                        )
                    })
                {
                    synthetic_calls.push(synthetic_call);
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
        expr: String,
        location: Location,
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

        let metadata = SdkMethodCallMetadata::new(expr.clone(), location)
            .with_parameters(parameters)
            .with_receiver(constructor.variable_name.clone()); // Use actual variable name from constructor

        Some(SdkMethodCall {
            name: has_many_spec.operation.to_case(Case::Snake),
            possible_services: vec![constructor.service_name.clone()],
            metadata: Some(metadata),
        })
    }

    /// New Tier 3: Find unmatched utility methods and collection accesses (conservative fallback)
    ///
    /// Searches for method calls and attribute accesses that match utility/collection patterns
    /// but were NOT matched in Tiers 1/2 (unknown receivers). Generates synthetics with
    /// all-synthetic parameters since we don't know the receiver.
    fn find_unmatched_utility_and_collection_calls(
        &self,
        ast: &AstWithSourceFile<Python>,
        matched_locations: &HashSet<Location>,
    ) -> Vec<SdkMethodCall> {
        let mut tier3_calls = Vec::new();

        // Search for utility method calls across all services
        tier3_calls.extend(self.find_unmatched_utility_method_calls(ast, matched_locations));

        // Search for collection accesses across all services
        tier3_calls.extend(self.find_unmatched_collection_accesses(ast, matched_locations));

        tier3_calls
    }

    /// Find utility method calls with unknown receivers (Tier 3)
    fn find_unmatched_utility_method_calls(
        &self,
        ast: &AstWithSourceFile<Python>,
        matched_locations: &HashSet<Location>,
    ) -> Vec<SdkMethodCall> {
        let root = ast.ast.root();
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

                let location =
                    Location::from_node(ast.source_file.path.clone(), node_match.get_node());

                // Skip if already matched in Tier 1/2
                if matched_locations.contains(&location) {
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
                                node_match.text().to_string(),
                                &location,
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
                                    node_match.text().to_string(),
                                    &location,
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
        ast: &AstWithSourceFile<Python>,
        matched_locations: &HashSet<Location>,
    ) -> Vec<SdkMethodCall> {
        let root = ast.ast.root();
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

                let location =
                    Location::from_node(ast.source_file.path.clone(), node_match.get_node());

                // Skip if already matched in Tier 1/2
                if matched_locations.contains(&location) {
                    continue;
                }

                // Search for this collection name across all services
                for (service_name, boto3_model) in self.registry.models() {
                    // Check all resource types for hasMany collections (resource-level)
                    for resource_def in boto3_model.get_all_resource_definitions().values() {
                        if let Some(has_many_spec) = resource_def.has_many.get(&attr_name) {
                            // Generate synthetic with all-synthetic parameters
                            let metadata =
                                SdkMethodCallMetadata::new(
                                    node_match.text().to_string(),
                                    location.clone(),
                                )
                                .with_parameters(self.generate_synthetic_parameters(
                                    &has_many_spec.identifier_params,
                                ))
                                .with_receiver(receiver_var.clone()); // Use actual receiver from code

                            calls.push(SdkMethodCall {
                                name: has_many_spec.operation.to_case(Case::Snake),
                                possible_services: vec![service_name.clone()],
                                metadata: Some(metadata),
                            });
                        }
                    }

                    // Check service-level hasMany collections
                    if let Some(service_has_many_spec) = boto3_model
                        .get_service_has_many_collections()
                        .get(&attr_name)
                    {
                        // Generate synthetic with all-synthetic parameters (service-level collections typically have no identifier params)
                        let metadata = SdkMethodCallMetadata::new(
                            node_match.text().to_string(),
                            location.clone(),
                        )
                        .with_parameters(self.generate_synthetic_parameters(
                            &service_has_many_spec.identifier_params,
                        ))
                        .with_receiver(receiver_var.clone()); // Use actual receiver from code

                        calls.push(SdkMethodCall {
                            name: service_has_many_spec.operation.to_case(Case::Snake),
                            possible_services: vec![service_name.clone()],
                            metadata: Some(metadata),
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
        expr: String,
        location: &Location,
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

        let metadata = SdkMethodCallMetadata::new(expr.clone(), location.clone())
            .with_parameters(parameters)
            .with_receiver(receiver_marker.to_string());

        SdkMethodCall {
            name: operation.to_case(Case::Snake),
            possible_services: vec![service_name.to_string()],
            metadata: Some(metadata),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Language, SourceFile};
    use ast_grep_core::tree_sitter::LanguageExt;
    use rstest::rstest;
    use std::path::PathBuf;

    fn create_test_ast(source_code: &str) -> AstWithSourceFile<Python> {
        let source_file =
            SourceFile::with_language(PathBuf::new(), source_code.to_string(), Language::Python);
        let ast_grep = Python.ast_grep(&source_file.content);
        AstWithSourceFile::new(ast_grep, source_file.clone())
    }

    /// The value of the keyword parameter named `name`, or `None`. Returns the full
    /// [`ParameterValue`] so tests can assert the resolution kind (`Resolved` literal vs
    /// `Unresolved` expression), not just the text.
    fn keyword_value<'a>(call: &'a SdkMethodCall, name: &str) -> Option<&'a ParameterValue> {
        call.metadata
            .as_ref()?
            .parameters
            .iter()
            .find_map(|p| match p {
                Parameter::Keyword { name: n, value, .. } if n == name => Some(value),
                _ => None,
            })
    }

    /// A chain's leaf operation must receive each identifier's *own* value — distinct
    /// values for distinct identifiers, accumulated correctly across navigations, and
    /// preserving each value's resolution kind. This asserts injected parameter VALUES
    /// directly (not just the operation name), which is the only level at which the
    /// per-identifier mapping is observable: disambiguation validates parameter names,
    /// not values.
    #[rstest]
    // Two-identifier leaf: Bucket -> Object carries the bucket name and takes the key.
    #[case::bucket_object_put(
        r#"
import boto3
def handle():
    s3 = boto3.resource("s3")
    s3.Bucket("my-bucket").Object("my-key").put(Body="data")
"#,
        "put_object",
        &[
            ("Bucket", ParameterValue::Resolved("my-bucket".into())),
            ("Key", ParameterValue::Resolved("my-key".into())),
        ]
    )]
    // Non-literal arguments (a variable and a function call) must map to the correct
    // identifiers AND stay unresolved — the chain logic carries the expression verbatim.
    #[case::non_literal_values(
        r#"
import boto3
def handle(bucket_var):
    s3 = boto3.resource("s3")
    s3.Bucket(bucket_var).Object(get_key()).put(Body="data")
"#,
        "put_object",
        &[
            ("Bucket", ParameterValue::Unresolved("bucket_var".into())),
            ("Key", ParameterValue::Unresolved("get_key()".into())),
        ]
    )]
    // Depth-3 chain accumulating three distinct identifiers into UploadPart.
    #[case::deep_multipart_upload_part(
        r#"
import boto3
def handle():
    s3 = boto3.resource("s3")
    s3.Object("my-bucket", "my-key").MultipartUpload("upload-id").Part(1).upload(Body="data")
"#,
        "upload_part",
        &[
            ("Bucket", ParameterValue::Resolved("my-bucket".into())),
            ("Key", ParameterValue::Resolved("my-key".into())),
            ("UploadId", ParameterValue::Resolved("upload-id".into())),
        ]
    )]
    #[tokio::test]
    async fn chain_injects_distinct_identifier_values(
        #[case] source: &str,
        #[case] operation: &str,
        #[case] expected: &[(&str, ParameterValue)],
    ) {
        let service_index = ServiceDiscovery::load_service_index(Language::Python)
            .await
            .expect("failed to load service index");
        let extractor = ResourceDirectCallsExtractor::new(&service_index);

        let ast = create_test_ast(source);
        let calls = extractor.extract_resource_method_calls(&ast);
        let call = calls
            .iter()
            .find(|c| c.name == operation)
            .unwrap_or_else(|| panic!("{operation} should be extracted, got: {calls:?}"));

        for (name, value) in expected {
            assert_eq!(
                keyword_value(call, name),
                Some(value),
                "{name} should carry its own value {value:?}; call: {call:?}"
            );
        }
    }
}
