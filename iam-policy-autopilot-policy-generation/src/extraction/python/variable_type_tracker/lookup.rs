use super::types::{VariableTypeInfo, VariableTypeTracker};
use std::collections::HashSet;

impl VariableTypeTracker {
    /// Look up the service type for a variable name
    ///
    /// Returns the service name if the variable is a tracked boto3 client/resource
    ///
    /// Checks in Python scoping order (LEGB - Local, Enclosing, Global, Built-in):
    /// 1. Function scope (local variables and parameters)
    /// 2. Module scope (global variables)
    ///
    /// # Note on Parameter Types
    /// When checking parameter types, this returns the first service found in the set.
    /// For parameters with multiple possible types, use `get_services_for_parameter()`.
    pub(crate) fn get_service_for_variable_in_context(
        &self,
        var_name: &str,
        function_name: Option<&str>,
    ) -> Option<&String> {
        if let Some(func_name) = function_name {
            if let Some(func_scope) = self.function_scopes.get(func_name) {
                if let Some(type_info) = func_scope.get(var_name) {
                    return Some(&type_info.service_name);
                }
            }

            if let Some(type_infos) = self
                .parameter_types
                .get(&(func_name.to_string(), var_name.to_string()))
            {
                return type_infos.iter().next().map(|info| &info.service_name);
            }
        }

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
        if let Some(func_name) = function_name {
            if let Some(func_scope) = self.function_scopes.get(func_name) {
                if let Some(type_info) = func_scope.get(var_name) {
                    return Some(type_info);
                }
            }

            if let Some(type_infos) = self
                .parameter_types
                .get(&(func_name.to_string(), var_name.to_string()))
            {
                if let Some(type_info) = type_infos.iter().next() {
                    return Some(type_info);
                }
            }
        }

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
