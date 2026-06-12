use super::types::{SdkObjectKind, VariableTypeInfo, VariableTypeTracker};
use crate::extraction::python::node_kinds;
use crate::extraction::AstWithSourceFile;
use ast_grep_language::Python;
use std::collections::{HashMap, HashSet};

impl VariableTypeTracker {
    /// Track boto3.client() and boto3.resource() assignments in the AST
    ///
    /// This is the main entry point that orchestrates all tracking patterns:
    ///
    /// 1. **Client assignments**: `boto3.client('service')` at module and function level
    /// 2. **Resource assignments**: `boto3.resource('service')` at module and function level
    /// 3. **Session-based assignments**: `session = boto3.Session(...)` then `session.client('service')`
    /// 4. **Aliases**: `my_client = s3_client` at module and function level
    /// 5. **Function calls**: Infer parameter types from arguments at call sites
    /// 6. **Resource-derived variables**: `table = dynamodb.Table('name')`, `bucket = s3.Bucket('name')`
    pub(crate) fn track_boto3_assignments(&mut self, ast: &AstWithSourceFile<Python>) {
        let root = ast.ast.root();

        self.detect_conflicted_function_names(&root);
        self.collect_local_assignment_targets(&root);
        self.track_boto3_factory_assignments(&root, "client", SdkObjectKind::Client);
        self.track_boto3_factory_assignments(&root, "resource", SdkObjectKind::Resource);
        self.track_session_variables(&root);
        self.track_session_factory_assignments(&root, "client", SdkObjectKind::Client);
        self.track_session_factory_assignments(&root, "resource", SdkObjectKind::Resource);
        self.track_aliases(&root);
        self.track_function_calls(&root);
        self.track_resource_derived_variables(&root);
    }

    /// Detect top-level function names that appear multiple times in the file.
    /// These are marked as conflicted so lookups return None rather than
    /// picking an arbitrary scope (which could cause false narrowing).
    ///
    /// Methods inside class definitions are excluded — same-named methods in
    /// different classes (e.g., `S3Handler.process` and `EC2Handler.process`)
    /// don't conflict because `self.method()` calls won't match bare function
    /// names in `func_params`.
    fn detect_conflicted_function_names(
        &mut self,
        root: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) {
        let mut seen: HashSet<String> = HashSet::new();
        let func_def_pattern = "def $FUNC($$$): $$$BODY";

        for func_match in root.find_all(func_def_pattern) {
            // Skip methods — their immediate parent is a class body
            if is_method(&func_match) {
                continue;
            }

            let env = func_match.get_env();
            if let Some(node) = env.get_match("FUNC") {
                let name = node.text().to_string();
                if !seen.insert(name.clone()) {
                    self.conflicted_functions.insert(name);
                }
            }
        }

        if !self.conflicted_functions.is_empty() {
            log::debug!(
                "Detected conflicted function names: {:?}",
                self.conflicted_functions
            );
        }
    }

    /// Collect all assignment target variable names per function.
    /// Used to detect when a function locally shadows a module-level variable name.
    fn collect_local_assignment_targets(
        &mut self,
        root: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) {
        let func_def_pattern = "def $FUNC($$$): $$$BODY";
        let assign_pattern = "$VAR = $$$RHS";

        for func_match in root.find_all(func_def_pattern) {
            let env = func_match.get_env();
            let func_name = if let Some(node) = env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };

            let caller_node_id = func_match.get_node().node_id();
            for node_match in func_match.get_node().find_all(assign_pattern) {
                if has_intervening_function(&node_match, caller_node_id) {
                    continue;
                }

                let assign_env = node_match.get_env();
                if let Some(var_node) = assign_env.get_match("VAR") {
                    if var_node.kind() == node_kinds::IDENTIFIER {
                        self.local_assignments
                            .entry(func_name.clone())
                            .or_default()
                            .insert(var_node.text().to_string());
                    }
                }
            }
        }
    }

    /// Track boto3 factory assignments (client or resource) at both function and module level.
    fn track_boto3_factory_assignments(
        &mut self,
        root: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
        factory_method: &str,
        kind: SdkObjectKind,
    ) {
        let assign_pattern = format!("$VAR = boto3.{factory_method}($$$ARGS)");

        // First, track function-level assignments
        let func_def_pattern = "def $FUNC($$$): $$$BODY";
        for func_match in root.find_all(func_def_pattern) {
            let env = func_match.get_env();

            let func_name = if let Some(node) = env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };

            let caller_node_id = func_match.get_node().node_id();
            for node_match in func_match.get_node().find_all(assign_pattern.as_str()) {
                if has_intervening_function(&node_match, caller_node_id) {
                    continue;
                }

                let assign_env = node_match.get_env();

                let var_name = if let Some(var_node) = assign_env.get_match("VAR") {
                    var_node.text().to_string()
                } else {
                    continue;
                };

                let Some(service_name) = Self::extract_first_positional_string_arg(assign_env)
                else {
                    continue;
                };

                log::debug!(
                    "Tracked boto3.{factory_method} assignment in function '{func_name}': {var_name} -> {service_name}"
                );

                self.function_scopes
                    .entry(func_name.clone())
                    .or_default()
                    .insert(
                        var_name,
                        VariableTypeInfo::from_service_with_kind(service_name, kind.clone()),
                    );
            }
        }

        // Then track module-level assignments
        for node_match in root.find_all(assign_pattern.as_str()) {
            let env = node_match.get_env();

            if is_inside_function(&node_match) {
                continue;
            }

            let var_name = if let Some(var_node) = env.get_match("VAR") {
                var_node.text().to_string()
            } else {
                continue;
            };

            let Some(service_name) = Self::extract_first_positional_string_arg(env) else {
                continue;
            };

            log::debug!(
                "Tracked boto3.{factory_method} assignment at module level: {var_name} -> {service_name}"
            );
            self.module_scope.insert(
                var_name,
                VariableTypeInfo::from_service_with_kind(service_name, kind.clone()),
            );
        }
    }

    /// Extract the first positional string argument from a matched `$$$ARGS` list.
    /// Skips keyword arguments and commas; returns the unquoted string content.
    fn extract_first_positional_string_arg(
        env: &ast_grep_core::meta_var::MetaVarEnv<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Option<String> {
        let arg_nodes = env.get_multiple_matches("ARGS");
        for arg_node in &arg_nodes {
            let text = arg_node.text().to_string();
            let trimmed = text.trim();
            if trimmed == "," || trimmed.is_empty() {
                continue;
            }
            if arg_node.kind() == node_kinds::KEYWORD_ARGUMENT {
                continue;
            }
            return Some(Self::extract_string_literal(trimmed));
        }
        None
    }

    /// Track `boto3.Session(...)` assignments to identify session variables
    fn track_session_variables(
        &mut self,
        root: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) {
        let pattern = "$VAR = boto3.Session($$$ARGS)";

        // Track function-level session variables
        let func_def_pattern = "def $FUNC($$$): $$$BODY";
        for func_match in root.find_all(func_def_pattern) {
            let env = func_match.get_env();
            let func_name = if let Some(node) = env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };

            let caller_node_id = func_match.get_node().node_id();
            for node_match in func_match.get_node().find_all(pattern) {
                if has_intervening_function(&node_match, caller_node_id) {
                    continue;
                }

                let assign_env = node_match.get_env();
                let var_name = if let Some(var_node) = assign_env.get_match("VAR") {
                    var_node.text().to_string()
                } else {
                    continue;
                };

                log::debug!(
                    "Tracked boto3.Session assignment in function '{func_name}': {var_name}"
                );
                self.session_variables
                    .entry(Some(func_name.clone()))
                    .or_default()
                    .insert(var_name);
            }
        }

        // Track module-level session variables
        for node_match in root.find_all(pattern) {
            if is_inside_function(&node_match) {
                continue;
            }

            let env = node_match.get_env();
            let var_name = if let Some(var_node) = env.get_match("VAR") {
                var_node.text().to_string()
            } else {
                continue;
            };

            log::debug!("Tracked boto3.Session assignment at module level: {var_name}");
            self.session_variables
                .entry(None)
                .or_default()
                .insert(var_name);
        }
    }

    /// Track `session.client('service')` / `session.resource('service')` assignments
    /// where `session` is a known boto3.Session() variable in the same scope.
    fn track_session_factory_assignments(
        &mut self,
        root: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
        factory_method: &str,
        kind: SdkObjectKind,
    ) {
        let assign_pattern = format!("$VAR = $SESSION.{factory_method}($$$ARGS)");

        // Track function-level assignments
        let func_def_pattern = "def $FUNC($$$): $$$BODY";
        for func_match in root.find_all(func_def_pattern) {
            let env = func_match.get_env();

            let func_name = if let Some(node) = env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };

            let caller_node_id = func_match.get_node().node_id();
            for node_match in func_match.get_node().find_all(assign_pattern.as_str()) {
                if has_intervening_function(&node_match, caller_node_id) {
                    continue;
                }

                let assign_env = node_match.get_env();

                let session_name = if let Some(node) = assign_env.get_match("SESSION") {
                    node.text().to_string()
                } else {
                    continue;
                };

                // Check function scope first, then fall back to module scope.
                // Skip module fallback if the name is locally assigned within this
                // function (shadowed), even if it's not a boto3 assignment we track.
                let in_func_sessions = self
                    .session_variables
                    .get(&Some(func_name.clone()))
                    .is_some_and(|vars| vars.contains(&session_name));

                if !in_func_sessions {
                    let locally_shadowed = self
                        .local_assignments
                        .get(&func_name)
                        .is_some_and(|vars| vars.contains(&session_name));
                    let in_module_sessions = !locally_shadowed
                        && self
                            .session_variables
                            .get(&None)
                            .is_some_and(|vars| vars.contains(&session_name));
                    if !in_module_sessions {
                        continue;
                    }
                }

                let var_name = if let Some(var_node) = assign_env.get_match("VAR") {
                    var_node.text().to_string()
                } else {
                    continue;
                };

                let Some(service_name) = Self::extract_first_positional_string_arg(assign_env)
                else {
                    continue;
                };

                log::debug!(
                    "Tracked {session_name}.{factory_method} assignment in function '{func_name}': {var_name} -> {service_name}"
                );

                self.function_scopes
                    .entry(func_name.clone())
                    .or_default()
                    .insert(
                        var_name,
                        VariableTypeInfo::from_service_with_kind(service_name, kind.clone()),
                    );
            }
        }

        // Track module-level assignments
        for node_match in root.find_all(assign_pattern.as_str()) {
            let env = node_match.get_env();

            if is_inside_function(&node_match) {
                continue;
            }

            let session_name = if let Some(node) = env.get_match("SESSION") {
                node.text().to_string()
            } else {
                continue;
            };

            // Module-level: only check module-scope sessions
            let is_known_session = self
                .session_variables
                .get(&None)
                .is_some_and(|vars| vars.contains(&session_name));

            if !is_known_session {
                continue;
            }

            let var_name = if let Some(var_node) = env.get_match("VAR") {
                var_node.text().to_string()
            } else {
                continue;
            };

            let Some(service_name) = Self::extract_first_positional_string_arg(env) else {
                continue;
            };

            log::debug!(
                "Tracked {session_name}.{factory_method} assignment at module level: {var_name} -> {service_name}"
            );
            self.module_scope.insert(
                var_name,
                VariableTypeInfo::from_service_with_kind(service_name, kind.clone()),
            );
        }
    }

    /// Track simple variable aliases at module and function level
    /// Pattern: `my_client = s3_client` where s3_client is already tracked
    fn track_aliases(
        &mut self,
        root: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) {
        // Module-level aliases first, so they're available when function-level aliases
        // reference them via get_type_info_for_variable_in_context fallback to module scope.
        let pattern = "$NEW = $OLD";
        for node_match in root.find_all(pattern) {
            let env = node_match.get_env();

            if is_inside_function(&node_match) {
                continue;
            }

            let old_node = if let Some(node) = env.get_match("OLD") {
                node
            } else {
                continue;
            };

            if old_node.kind() != node_kinds::IDENTIFIER {
                continue;
            }

            let new_var = if let Some(node) = env.get_match("NEW") {
                node.text().to_string()
            } else {
                continue;
            };

            let old_var = old_node.text().to_string();

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

        // Then function-level aliases (can now reference module-level aliases)
        let func_def_pattern = "def $FUNC($$$PARAMS):$$$BODY";

        for func_match in root.find_all(func_def_pattern) {
            let env = func_match.get_env();

            let func_name = if let Some(node) = env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };

            let caller_node_id = func_match.get_node().node_id();
            for node_match in func_match.get_node().find_all(pattern) {
                if has_intervening_function(&node_match, caller_node_id) {
                    continue;
                }
                let assign_env = node_match.get_env();

                let old_node = if let Some(node) = assign_env.get_match("OLD") {
                    node
                } else {
                    continue;
                };

                if old_node.kind() != node_kinds::IDENTIFIER {
                    continue;
                }

                let new_var = if let Some(node) = assign_env.get_match("NEW") {
                    node.text().to_string()
                } else {
                    continue;
                };

                let old_var = old_node.text().to_string();

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
                // Skip variadic params (*args, **kwargs) — they shouldn't participate
                // in positional matching since they collect remaining arguments.
                if param_text.starts_with('*') {
                    continue;
                }
                if let Some(param_name) = Self::extract_all_params(&param_text).into_iter().next() {
                    if param_name != "self" && param_name != "cls" {
                        params.push(param_name);
                    }
                }
            }

            if !params.is_empty() {
                log::debug!("Found function definition: {func_name}({params:?})");
                func_params.insert(func_name, params);
            }
        }

        log::debug!("Function parameters map: {func_params:?}");

        // Find function calls within function bodies (with calling context).
        // Skip calls inside nested function definitions to avoid wrong-context attribution.
        let call_pattern = "$FUNC($$$ARGS)";
        let caller_func_pattern = "def $FUNC($$$PARAMS):$$$BODY";

        for func_match in root.find_all(caller_func_pattern) {
            let caller_env = func_match.get_env();
            let caller_name = if let Some(node) = caller_env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };

            let caller_node_id = func_match.get_node().node_id();

            for node_match in func_match.get_node().find_all(call_pattern) {
                // Skip calls that are inside a nested function (they'll be handled
                // when we iterate that inner function as its own caller).
                if has_intervening_function(&node_match, caller_node_id) {
                    continue;
                }

                let env = node_match.get_env();

                let func_name = if let Some(node) = env.get_match("FUNC") {
                    node.text().to_string()
                } else {
                    continue;
                };

                let arg_nodes = env.get_multiple_matches("ARGS");
                self.match_call_args_to_params(
                    &func_name,
                    &arg_nodes,
                    &func_params,
                    Some(&caller_name),
                );
            }
        }

        // Find module-level function calls (no calling context)
        for node_match in root.find_all(call_pattern) {
            if is_inside_function(&node_match) {
                continue;
            }

            let env = node_match.get_env();

            let func_name = if let Some(node) = env.get_match("FUNC") {
                node.text().to_string()
            } else {
                continue;
            };

            let arg_nodes = env.get_multiple_matches("ARGS");
            self.match_call_args_to_params(&func_name, &arg_nodes, &func_params, None);
        }
    }

    /// Match call-site arguments to function parameter names and record type info
    fn match_call_args_to_params(
        &mut self,
        func_name: &str,
        arg_nodes: &[ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>],
        func_params: &HashMap<String, Vec<String>>,
        caller_context: Option<&str>,
    ) {
        let Some(param_names) = func_params.get(func_name) else {
            log::debug!("No parameter mapping found for function {func_name}");
            return;
        };

        let mut positional_index = 0;
        for arg_node in arg_nodes {
            let arg_text = arg_node.text().to_string();
            let trimmed = arg_text.trim();
            if trimmed == "," || trimmed.is_empty() {
                continue;
            }

            if arg_node.kind() == node_kinds::KEYWORD_ARGUMENT {
                if let Some(eq_pos) = arg_text.find('=') {
                    let key = arg_text[..eq_pos].trim();
                    let value = arg_text[eq_pos + 1..].trim();
                    if param_names.contains(&key.to_string()) {
                        let type_info = self
                            .get_type_info_for_variable_in_context(value, caller_context)
                            .cloned();

                        if let Some(type_info) = type_info {
                            log::debug!(
                                "Tracked function call: {}({}={}) - keyword param '{}' -> service '{}'",
                                func_name,
                                key,
                                value,
                                key,
                                type_info.service_name
                            );
                            self.parameter_types
                                .entry((func_name.to_string(), key.to_string()))
                                .or_default()
                                .insert(type_info);
                        }
                    }
                }
            } else {
                if let Some(param_name) = param_names.get(positional_index) {
                    let type_info = self
                        .get_type_info_for_variable_in_context(trimmed, caller_context)
                        .cloned();

                    if let Some(type_info) = type_info {
                        log::debug!(
                            "Tracked function call: {}({}) - param '{}' (position {}) -> service '{}'",
                            func_name,
                            trimmed,
                            param_name,
                            positional_index,
                            type_info.service_name
                        );
                        self.parameter_types
                            .entry((func_name.to_string(), param_name.clone()))
                            .or_default()
                            .insert(type_info);
                    }
                }
                positional_index += 1;
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

                // Strip leading * or ** (e.g., "*args" -> "args", "**kwargs" -> "kwargs")
                // A bare "*" (keyword-only separator) becomes empty and is filtered out
                let param = param.trim_start_matches('*');

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

            let caller_node_id = func_match.get_node().node_id();
            let pattern = "$VAR = $RESOURCE.$METHOD($$$ARGS)";
            for node_match in func_match.get_node().find_all(pattern) {
                if has_intervening_function(&node_match, caller_node_id) {
                    continue;
                }

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
    pub(super) fn extract_string_literal(raw: &str) -> String {
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

/// Check if there is a function_definition node between the matched node and
/// the specified ancestor (identified by node_id). This detects calls inside
/// nested functions when iterating from an outer function's subtree.
fn has_intervening_function(
    node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ancestor_node_id: usize,
) -> bool {
    let mut current = node_match.get_node().parent();
    while let Some(node) = current {
        if node.node_id() == ancestor_node_id {
            return false;
        }
        if node.kind() == node_kinds::FUNCTION_DEFINITION {
            return true;
        }
        current = node.parent();
    }
    false
}

/// Check if a function definition is a method (immediate child of a class body)
fn is_method(
    func_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
) -> bool {
    let mut current = func_match.get_node().parent();
    while let Some(node) = current {
        let kind = node.kind();
        if kind == node_kinds::CLASS_DEFINITION {
            return true;
        }
        // Walk through intermediate nodes (block, decorated_definition)
        if kind == node_kinds::BLOCK || kind == node_kinds::DECORATED_DEFINITION {
            current = node.parent();
            continue;
        }
        return false;
    }
    false
}
