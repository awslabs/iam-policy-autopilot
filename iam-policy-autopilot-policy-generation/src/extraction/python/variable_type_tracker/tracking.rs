use super::types::{SdkObjectKind, VariableTypeInfo, VariableTypeTracker};
use crate::extraction::python::node_kinds;
use crate::extraction::AstWithSourceFile;
use ast_grep_language::Python;
use std::collections::HashMap;

impl VariableTypeTracker {
    /// Track boto3.client() and boto3.resource() assignments in the AST
    ///
    /// This is the main entry point that orchestrates all tracking patterns:
    ///
    /// 1. **Client assignments**: `boto3.client('service')` at module and function level
    /// 2. **Resource assignments**: `boto3.resource('service')` at module and function level
    /// 3. **Aliases**: `my_client = s3_client` at module and function level
    /// 4. **Function calls**: Infer parameter types from arguments at call sites
    /// 5. **Resource-derived variables**: `table = dynamodb.Table('name')`, `bucket = s3.Bucket('name')`
    pub(crate) fn track_boto3_assignments(&mut self, ast: &AstWithSourceFile<Python>) {
        let root = ast.ast.root();

        self.track_client_assignments(&root);
        self.track_resource_assignments(&root);
        self.track_aliases(&root);
        self.track_function_calls(&root);
        self.track_resource_derived_variables(&root);
    }

    /// Track boto3.client() assignments
    fn track_client_assignments(
        &mut self,
        root: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) {
        // First, track function-level assignments
        let func_def_pattern = "def $FUNC($$$): $$$BODY";
        for func_match in root.find_all(func_def_pattern) {
            log::debug!("FUNC_MATCH: {}", func_match.text());
            let env = func_match.get_env();

            let func_name = if let Some(node) = env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };

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
                    "Tracked boto3.client assignment in function '{func_name}': {var_name} -> {service_name}"
                );

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

            if is_inside_function(&node_match) {
                continue;
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
                "Tracked boto3.client assignment at module level: {var_name} -> {service_name}"
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
                    "Tracked boto3.resource assignment in function '{func_name}': {var_name} -> {service_name}"
                );

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

            if is_inside_function(&node_match) {
                continue;
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
                "Tracked boto3.resource assignment at module level: {var_name} -> {service_name}"
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

                    self.function_scopes
                        .entry(func_name.clone())
                        .or_default()
                        .insert(new_var, type_info);
                }
            }
        }

        // Also track module-level aliases
        let pattern = "$NEW = $OLD";
        for node_match in root.find_all(pattern) {
            let env = node_match.get_env();

            if is_inside_function(&node_match) {
                continue;
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
        // Build a map of function names to their parameter lists
        let mut func_params: HashMap<String, Vec<String>> = HashMap::new();

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
                if param_text == "," || param_text.is_empty() {
                    continue;
                }
                if let Some(param_name) = Self::extract_all_params(&param_text).into_iter().next() {
                    params.push(param_name);
                }
            }

            if !params.is_empty() {
                log::debug!("Found function definition: {func_name}({params:?})");
                func_params.insert(func_name, params);
            }
        }

        log::debug!("Function parameters map: {func_params:?}");

        // Find function calls and match arguments to parameters
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
                .filter(|arg| arg != "," && !arg.is_empty())
                .collect();

            if args.is_empty() {
                continue;
            }

            log::debug!("Found function call: {func_name}({args:?})");

            if let Some(param_names) = func_params.get(&func_name) {
                for (i, arg) in args.iter().enumerate() {
                    if let Some(param_name) = param_names.get(i) {
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
                            self.parameter_types
                                .entry((func_name.clone(), param_name.clone()))
                                .or_default()
                                .insert(type_info);
                        }
                    }
                }
            } else {
                log::debug!("No parameter mapping found for function {func_name}");
            }
        }
    }

    /// Extract all parameters/arguments from a comma-separated list
    ///
    /// Handles:
    /// - Default values: "client=None" -> "client"
    /// - Type annotations: "client: str" -> "client"
    /// - Whitespace: " client , bucket " -> vec!["client", "bucket"]
    pub(super) fn extract_all_params(params: &str) -> Vec<String> {
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

                if method_name == "get_paginator" || method_name == "get_waiter" {
                    continue;
                }

                if let Some(func_scope) = self.function_scopes.get(&func_name) {
                    if func_scope.contains_key(&var_name) {
                        continue;
                    }
                }

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
        let pattern = "$VAR = $RESOURCE.$METHOD($$$ARGS)";

        for node_match in root.find_all(pattern) {
            let env = node_match.get_env();

            if is_inside_function(&node_match) {
                continue;
            }

            let var_name = if let Some(node) = env.get_match("VAR") {
                node.text().to_string()
            } else {
                continue;
            };

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

            if method_name == "get_paginator" || method_name == "get_waiter" {
                continue;
            }

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
    pub(super) fn extract_string_literal(&self, raw: &str) -> String {
        raw.trim()
            .trim_start_matches('\'')
            .trim_start_matches('"')
            .trim_end_matches('\'')
            .trim_end_matches('"')
            .to_string()
    }
}

/// Check if a matched node is inside a function definition
fn is_inside_function(
    node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
) -> bool {
    let mut current = node_match.get_node().parent();
    while let Some(node) = current {
        if node.kind() == node_kinds::FUNCTION_DEFINITION {
            return true;
        }
        current = node.parent();
    }
    false
}
