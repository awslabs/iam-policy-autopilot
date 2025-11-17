//! Core JavaScript/TypeScript scanning logic for AWS SDK extraction

use crate::extraction::javascript::types::{
    ImportInfo, SublibraryInfo, ClientInstantiation, 
    ValidClientTypes, MethodCall, JavaScriptScanResults
};

use ast_grep_core::matcher::Pattern;
use ast_grep_core::MatchStrictness;

use std::collections::HashMap;

/// Parse import item with line number - standalone utility function
pub(crate) fn parse_import_item_with_line(import_item: &str, line: usize) -> Option<ImportInfo> {
    let import_item = import_item.trim();
    if import_item.is_empty() {
        return None;
    }

    // Check for rename syntax: "OriginalName as LocalName"
    if let Some(as_pos) = import_item.find(" as ") {
        let original_name = import_item[..as_pos].trim().to_string();
        let local_name = import_item[as_pos + 4..].trim().to_string();
        Some(ImportInfo::new(original_name, local_name, line))
    } else {
        // No rename - original name is the same as local name
        let import_name = import_item.trim().to_string();
        Some(ImportInfo::new(import_name.clone(), import_name, line))
    }
}

/// Parse object literal - standalone utility function
pub(crate) fn parse_object_literal(obj_text: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();

    if obj_text.trim().is_empty() {
        return result;
    }

    let obj_text = obj_text.trim();

    // Handle empty objects
    if obj_text == "{}" || obj_text == "()" {
        return result;
    }

    // Remove outer braces/parentheses if present
    let obj_text = if (obj_text.starts_with('{') && obj_text.ends_with('}'))
        || (obj_text.starts_with('(') && obj_text.ends_with(')'))
    {
        &obj_text[1..obj_text.len() - 1]
    } else {
        obj_text
    };

    // Simple parsing for key-value pairs
    let mut current_pair = String::new();
    let mut quote_char = None;
    let mut paren_level = 0;

    for ch in obj_text.chars() {
        match ch {
            '"' | '\'' if quote_char.is_none() => {
                quote_char = Some(ch);
                current_pair.push(ch);
            }
            ch if Some(ch) == quote_char => {
                quote_char = None;
                current_pair.push(ch);
            }
            '(' | '{' | '[' if quote_char.is_none() => {
                paren_level += 1;
                current_pair.push(ch);
            }
            ')' | '}' | ']' if quote_char.is_none() => {
                paren_level -= 1;
                current_pair.push(ch);
            }
            ',' if quote_char.is_none() && paren_level == 0 => {
                parse_key_value_pair(current_pair.trim(), &mut result);
                current_pair.clear();
            }
            _ => {
                current_pair.push(ch);
            }
        }
    }

    // Handle the last pair
    if !current_pair.trim().is_empty() {
        parse_key_value_pair(current_pair.trim(), &mut result);
    }

    result
}

/// Parse key-value pair - standalone utility function
fn parse_key_value_pair(pair: &str, result: &mut HashMap<String, String>) {
    if let Some(colon_pos) = pair.find(':') {
        let key = pair[..colon_pos]
            .trim()
            .trim_matches('"')
            .trim_matches('\'');
        let value = pair[colon_pos + 1..]
            .trim()
            .trim_matches('"')
            .trim_matches('\'');

        // Try to convert boolean/numeric values
        let final_value = match value.to_lowercase().as_str() {
            "true" => "true".to_string(),
            "false" => "false".to_string(),
            _ => {
                if value.parse::<i64>().is_ok() || value.parse::<f64>().is_ok() {
                    value.to_string()
                } else {
                    value.to_string()
                }
            }
        };

        result.insert(key.to_string(), final_value);
    }
}

/// Parse and add imports from import text with line number - standalone utility function
fn parse_and_add_imports_with_line(imports_text: &str, sublibrary_info: &mut SublibraryInfo, line: usize) {
    // Handle different import formats
    if imports_text.starts_with('{') && imports_text.ends_with('}') {
        // Destructuring - parse with rename support
        let imports_content = &imports_text[1..imports_text.len() - 1]; // Remove braces

        // Split by comma and parse each import
        for import_item in imports_content.split(',') {
            if let Some(import_info) = parse_import_item_with_line(import_item, line) {
                sublibrary_info.add_import(import_info);
            }
        }
    } else {
        // Default import - single identifier
        if let Some(import_info) = parse_import_item_with_line(imports_text, line) {
            sublibrary_info.add_import(import_info);
        }
    }
}

/// Core AST scanner for JavaScript/TypeScript AWS SDK usage patterns
pub(crate) struct ASTScanner<T> 
where 
    T: ast_grep_core::Doc + Clone,
{
    /// Pre-built AST grep root passed from extractor
    ast_grep: ast_grep_core::AstGrep<T>,
    language: ast_grep_language::SupportLang,
}

impl<T> ASTScanner<T> 
where 
    T: ast_grep_core::Doc + Clone,
{
    /// Create a new scanner with pre-built AST from extractor
    pub(crate) fn new(ast_grep: ast_grep_core::AstGrep<T>, language: ast_grep_language::SupportLang) -> Self {
        Self {
            ast_grep,
            language
        }
    }

    /// Execute a pattern match against the AST - now generic for both JavaScript and TypeScript
    /// Uses relaxed strictness to handle inline comments between arguments
    fn find_all_matches(
        &self,
        pattern: &str,
    ) -> Result<Vec<ast_grep_core::NodeMatch<'_, T>>, String> {
        let root = self.ast_grep.root();
        
        // Build pattern with relaxed strictness to handle inline comments
        let pattern_obj = Pattern::new(pattern, self.language)
            .with_strictness(MatchStrictness::Relaxed);
        
        Ok(root.find_all(pattern_obj).collect())
    }


    /// Find Command instantiation and extract its arguments
    /// Returns (line_number, parameters) tuple
    pub(crate) fn find_command_instantiation_with_args(&self, command_name: &str) -> Option<(usize, Vec<crate::extraction::Parameter>)> {
        use crate::extraction::javascript::argument_extractor::ArgumentExtractor;
        
        let pattern = format!("new {}($ARGS)", command_name);
        
        if let Ok(matches) = self.find_all_matches(&pattern) {
            if let Some(first_match) = matches.first() {
                let line = first_match.get_node().start_pos().line() + 1;
                let env = first_match.get_env();
                
                // Extract arguments from the ARGS node
                // env.get_match returns Option<&Node>, so pass directly
                let args_node = env.get_match("ARGS");
                let parameters = ArgumentExtractor::extract_object_parameters(args_node);
                
                return Some((line, parameters));
            }
        }
        None
    }

    /// Find paginate function call and extract operation parameters (2nd argument)
    /// Returns (line_number, parameters) tuple
    pub(crate) fn find_paginate_function_with_args(&self, function_name: &str) -> Option<(usize, Vec<crate::extraction::Parameter>)> {
        use crate::extraction::javascript::argument_extractor::ArgumentExtractor;
        
        // Use explicit two-argument pattern
        let pattern = format!("{}($ARG1, $ARG2)", function_name);
        
        if let Ok(matches) = self.find_all_matches(&pattern) {
            if let Some(first_match) = matches.first() {
                let line = first_match.get_node().start_pos().line() + 1;
                let env = first_match.get_env();
                
                // Extract parameters from second argument (ARG2 = operation params)
                let second_arg = env.get_match("ARG2");
                let parameters = ArgumentExtractor::extract_object_parameters(second_arg);
                
                return Some((line, parameters));
            }
        }
        None
    }

    /// Find waiter function call and extract operation parameters (2nd argument)
    /// Returns (line_number, parameters) tuple
    pub(crate) fn find_waiter_function_with_args(&self, function_name: &str) -> Option<(usize, Vec<crate::extraction::Parameter>)> {
        use crate::extraction::javascript::argument_extractor::ArgumentExtractor;
        
        // Try patterns with and without await keyword using explicit two-argument pattern
        let patterns = [
            format!("await {}($ARG1, $ARG2)", function_name),  // With await
            format!("{}($ARG1, $ARG2)", function_name),        // Without await
        ];
        
        for pattern in &patterns {
            if let Ok(matches) = self.find_all_matches(pattern) {
                if let Some(first_match) = matches.first() {
                    let line = first_match.get_node().start_pos().line() + 1;
                    let env = first_match.get_env();
                    
                    // Extract parameters from second argument (ARG2 = operation params)
                    let second_arg = env.get_match("ARG2");
                    let parameters = ArgumentExtractor::extract_object_parameters(second_arg);
                    
                    return Some((line, parameters));
                }
            }
        }
        None
    }

    /// Find the position of CommandInput type usage (TypeScript-specific)
    /// Searches for patterns like `const params: QueryCommandInput = {...}` and returns the line number
    pub(crate) fn find_command_input_usage_position(&self, type_name: &str) -> Option<usize> {
        // Try multiple patterns for TypeScript type annotations
        let patterns = [
            format!("const $VAR: {} = $VALUE", type_name),   // const variable: Type = value
            format!("let $VAR: {} = $VALUE", type_name),     // let variable: Type = value  
            format!("$VAR: {} = $VALUE", type_name),         // variable: Type = value
        ];
        
        for pattern in &patterns {
            if let Ok(matches) = self.find_all_matches(pattern) {
                if let Some(first_match) = matches.first() {
                    return Some(first_match.get_node().start_pos().line() + 1);
                }
            }
        }
        None
    }


    /// Scan AWS import/require statements generically
    fn scan_aws_statements(&self, pattern: &str) -> Result<Vec<SublibraryInfo>, String> {
        let mut sublibrary_data: HashMap<String, SublibraryInfo> = HashMap::new();

        let matches = self.find_all_matches(pattern)?;
        Self::process_import_matches(matches, &mut sublibrary_data, true)?;

        Ok(sublibrary_data.into_values().collect())
    }

    /// Generic processing for import/require matches - works for both JavaScript and TypeScript
    fn process_import_matches<U>(
        matches: Vec<ast_grep_core::NodeMatch<U>>,
        sublibrary_data: &mut HashMap<String, SublibraryInfo>,
        include_line_numbers: bool,
    ) -> Result<(), String> 
    where
        U: ast_grep_core::Doc + std::clone::Clone,
    {
        for node_match in matches {
            let env = node_match.get_env();

            let module_node = env.get_match("MODULE");
            let imports_node = env.get_match("IMPORTS");

            if let (Some(module_node), Some(imports_node)) = (module_node, imports_node) {
                let module_text_cow = module_node.text();
                let module_text = module_text_cow.trim_matches('"').trim_matches('\'');

                // Check if it's an AWS SDK statement
                if !module_text.starts_with("@aws-sdk/") {
                    continue;
                }

                let sublibrary = module_text.strip_prefix("@aws-sdk/").unwrap().to_string();
                let imports_text = imports_node.text();
                let imports_text_str = imports_text.as_ref(); // Convert Cow to &str

                // Initialize sublibrary data if not exists
                let sublibrary_info = sublibrary_data
                    .entry(sublibrary.clone())
                    .or_insert_with(|| SublibraryInfo::new(sublibrary));

                if include_line_numbers {
                    // Get line number from AST node  
                    let line = node_match.get_node().start_pos().line() + 1;
                    parse_and_add_imports_with_line(imports_text_str, sublibrary_info, line);
                } else {
                    parse_and_add_imports_with_line(imports_text_str, sublibrary_info, 1);
                }
            }
        }
        Ok(())
    }





    /// Scan for AWS SDK ES6 imports
    pub(crate) fn scan_aws_imports(&mut self) -> Result<Vec<SublibraryInfo>, String> {
        self.scan_aws_statements("import $IMPORTS from $MODULE")
    }

    /// Scan for AWS SDK CommonJS requires
    pub(crate) fn scan_aws_requires(&mut self) -> Result<Vec<SublibraryInfo>, String> {
        // Support multiple require patterns (const, let, var - both destructuring and default imports)
        const REQUIRE_PATTERNS: &[&str] = &[
            "const $IMPORTS = require($MODULE)",  // Destructuring: const { S3Client } = require(...)
            "let $IMPORTS = require($MODULE)",    // Destructuring: let { S3Client } = require(...)
            "var $IMPORTS = require($MODULE)",    // Destructuring: var { S3Client } = require(...) [legacy]
        ];
        
        let mut all_requires = Vec::new();
        
        for pattern in REQUIRE_PATTERNS {
            let mut requires = self.scan_aws_statements(pattern)?;
            all_requires.append(&mut requires);
        }
        
        Ok(all_requires)
    }

    /// Scan for both ES6 imports and CommonJS requires
    pub(crate) fn scan_all_aws_imports(
        &mut self,
    ) -> Result<(Vec<SublibraryInfo>, Vec<SublibraryInfo>), String> {
        let imports = self.scan_aws_imports()?;
        let requires = self.scan_aws_requires()?;
        Ok((imports, requires))
    }

    /// Get all valid client types from import information
    fn get_valid_client_types(&mut self) -> Result<ValidClientTypes, String> {
        let (imports, requires) = self.scan_all_aws_imports()?;
        let mut client_types = Vec::new();
        let mut name_mappings = HashMap::new();
        let mut sublibrary_mappings = HashMap::new();

        // Process both imports and requires
        for source_data in [imports, requires].iter() {
            for sublibrary_info in source_data {
                for import_info in &sublibrary_info.imports {
                    let original_name = &import_info.original_name;
                    let local_name = &import_info.local_name;

                    // Check if it's a client type (starts with uppercase, doesn't end with Command/CommandInput)
                    if original_name
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_uppercase())
                        && !original_name.ends_with("Command")
                        && !original_name.ends_with("CommandInput")
                    {
                        client_types.push(local_name.clone());
                        name_mappings.insert(local_name.clone(), original_name.clone());
                        sublibrary_mappings
                            .insert(local_name.clone(), sublibrary_info.sublibrary.clone());
                    }
                }
            }
        }

        Ok(ValidClientTypes::new(client_types, name_mappings, sublibrary_mappings))
    }


    /// Scan for AWS client instantiations
    pub(crate) fn scan_client_instantiations(
        &mut self,
    ) -> Result<Vec<ClientInstantiation>, String> {
        let client_info = self.get_valid_client_types()?;

        if client_info.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        // Patterns to match client instantiations
        const PATTERNS: &[&str] = &[
            "const $VAR = new $CLIENT($ARGS)",
            "let $VAR = new $CLIENT($ARGS)",
        ];

        for pattern in PATTERNS {
            let matches = self.find_all_matches(pattern)?;
            Self::process_client_instantiation_matches(
                matches,
                &client_info.client_types,
                &client_info.name_mappings,
                &client_info.sublibrary_mappings,
                &mut results,
            )?;
        }

        Ok(results)
    }

    /// Generic processing for client instantiation matches - works for both JavaScript and TypeScript
    fn process_client_instantiation_matches<U>(
        matches: Vec<ast_grep_core::NodeMatch<U>>,
        valid_client_types: &[String],
        client_name_mappings: &HashMap<String, String>,
        client_sublibrary_mappings: &HashMap<String, String>,
        results: &mut Vec<ClientInstantiation>,
    ) -> Result<(), String> 
    where
        U: ast_grep_core::Doc + std::clone::Clone,
    {
        for node_match in matches {
            let env = node_match.get_env();

            let var_node = env.get_match("VAR");
            let client_node = env.get_match("CLIENT");
            let args_node = env.get_match("ARGS");

            if let (Some(var_node), Some(client_node)) = (var_node, client_node) {
                let variable_name = var_node.text().to_string();
                let client_type = client_node.text().to_string();

                // Check if it's a valid AWS client type
                if valid_client_types.contains(&client_type) {
                    let original_client_type = client_name_mappings
                        .get(&client_type)
                        .cloned()
                        .unwrap_or_else(|| client_type.clone());
                    let sublibrary = client_sublibrary_mappings
                        .get(&client_type)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());

                    // Extract arguments
                    let arguments = if let Some(args_node) = args_node {
                        let args_text = args_node.text();
                        parse_object_literal(args_text.as_ref())
                    } else {
                        HashMap::new()
                    };

                    // Get line number
                    let line = node_match.get_node().start_pos().line() + 1;

                    results.push(ClientInstantiation {
                        variable: variable_name,
                        client_type,
                        original_client_type,
                        sublibrary,
                        arguments,
                        line,
                    });
                }
            }
        }
        Ok(())
    }

    /// Generic processing for method call matches - works for both JavaScript and TypeScript
    fn process_method_call_matches<U>(
        matches: Vec<ast_grep_core::NodeMatch<U>>,
        client_variables: &[String],
        client_info_map: &HashMap<String, (String, String, String)>,
        results: &mut Vec<MethodCall>,
    ) -> Result<(), String> 
    where
        U: ast_grep_core::Doc + std::clone::Clone,
    {
        for node_match in matches {
            let env = node_match.get_env();

            let var_node = env.get_match("VAR");
            let method_node = env.get_match("METHOD");
            let args_node = env.get_match("ARGS");

            if let (Some(var_node), Some(method_node)) = (var_node, method_node) {
                let variable_name = var_node.text().to_string();
                let method_name = method_node.text().to_string();

                // Check if it's a known client variable
                if client_variables.contains(&variable_name) {
                    let (client_type, original_client_type, client_sublibrary) =
                        client_info_map.get(&variable_name).unwrap();

                    // Extract arguments
                    let arguments = if let Some(args_node) = args_node {
                        let args_text = args_node.text();
                        parse_object_literal(args_text.as_ref())
                    } else {
                        HashMap::new()
                    };

                    // Get line number
                    let line = node_match.get_node().start_pos().line() + 1;

                    results.push(MethodCall {
                        client_variable: variable_name,
                        client_type: client_type.clone(),
                        original_client_type: original_client_type.clone(),
                        client_sublibrary: client_sublibrary.clone(),
                        method_name,
                        arguments,
                        line,
                    });
                }
            }
        }
        Ok(())
    }



    /// Scan for method calls on AWS client instances
    pub(crate) fn scan_method_calls(&mut self) -> Result<Vec<MethodCall>, String> {
        let mut results = Vec::new();

        // Get client instantiation data to build client variable mapping
        let client_instantiations = self.scan_client_instantiations()?;
        if client_instantiations.is_empty() {
            return Ok(results);
        }

        // Create mapping from client variable to type/sublibrary info
        let client_info_map: HashMap<String, (String, String, String)> = client_instantiations
            .iter()
            .map(|c| {
                (
                    c.variable.clone(),
                    (
                        c.client_type.clone(),
                        c.original_client_type.clone(),
                        c.sublibrary.clone(),
                    ),
                )
            })
            .collect();

        let client_variables: Vec<String> = client_info_map.keys().cloned().collect();

        // Single pattern to match method calls (covers both awaited and non-awaited)
        let matches = self.find_all_matches("$VAR.$METHOD($ARGS)")?;
        Self::process_method_call_matches(
            matches,
            &client_variables,
            &client_info_map,
            &mut results,
        )?;

        Ok(results)
    }


    /// Perform all scanning operations and return combined results
    pub(crate) fn scan_all(&mut self) -> Result<JavaScriptScanResults, String> {
        let (imports, requires) = self.scan_all_aws_imports()?;
        let client_instantiations = self.scan_client_instantiations()?;

        // Scan for method calls on client instances
        let method_calls = self.scan_method_calls()?;

        Ok(JavaScriptScanResults {
            imports,
            requires,
            client_instantiations,
            method_calls,
        })
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use ast_grep_core::tree_sitter::LanguageExt;
    use ast_grep_language::{JavaScript, TypeScript};

    #[test]
    fn test_parse_import_item() {
        // Test regular import
        let import_info = parse_import_item_with_line("S3Client", 1).unwrap();
        assert_eq!(import_info.original_name, "S3Client");
        assert_eq!(import_info.local_name, "S3Client");
        assert!(!import_info.is_renamed);

        // Test renamed import
        let import_info = parse_import_item_with_line("S3Client as MyS3Client", 1).unwrap();
        assert_eq!(import_info.original_name, "S3Client");
        assert_eq!(import_info.local_name, "MyS3Client");
        assert!(import_info.is_renamed);
    }

    #[test]
    fn test_parse_object_literal() {
        let result = parse_object_literal("{region: 'us-east-1', timeout: 5000}");
        assert_eq!(result.get("region"), Some(&"us-east-1".to_string()));
        assert_eq!(result.get("timeout"), Some(&"5000".to_string()));

        // Test empty object
        let result = parse_object_literal("{}");
        assert!(result.is_empty());
    }

    #[test]
    fn test_import_require_scanning_comprehensive() {
        // Create comprehensive test case with multiple sublibrary patterns
        let source = r#"
import { S3Client, PutObjectCommand as PutObject, GetObjectCommand } from "@aws-sdk/client-s3";
import { DynamoDBClient as DynamoDB, QueryCommand } from "@aws-sdk/client-dynamodb";
import { paginateQuery, paginateScan as PaginateScanRenamed } from "@aws-sdk/lib-dynamodb";
const { LambdaClient, InvokeCommand } = require("@aws-sdk/client-lambda");
const { SESClient } = require("@aws-sdk/client-ses");
        "#;

        let ast = JavaScript.ast_grep(source);
        let mut scanner = ASTScanner::new(ast, JavaScript.into());
        let (imports, requires) = scanner.scan_all_aws_imports().unwrap();

        // === VERIFY BASIC COUNTS ===
        assert_eq!(imports.len(), 3, "Should find 3 ES6 import sublibraries");
        assert!(requires.len() >= 2, "Should find at least 2 CommonJS require sublibraries");

        // === VERIFY ES6 IMPORTS ===
        
        // Test client-s3 sublibrary
        let s3_sublibrary = imports.iter().find(|s| s.sublibrary == "client-s3")
            .expect("Should find client-s3 sublibrary");
        assert_eq!(s3_sublibrary.imports.len(), 3, "client-s3 should have 3 imports");
        
        // Verify S3Client import (no rename)
        let s3_client = s3_sublibrary.imports.iter()
            .find(|i| i.original_name == "S3Client")
            .expect("Should find S3Client import");
        assert_eq!(s3_client.local_name, "S3Client");
        assert!(!s3_client.is_renamed);
        
        // Verify PutObjectCommand import (with rename)
        let put_object = s3_sublibrary.imports.iter()
            .find(|i| i.original_name == "PutObjectCommand")
            .expect("Should find PutObjectCommand import");
        assert_eq!(put_object.local_name, "PutObject");
        assert!(put_object.is_renamed);
        
        // Verify GetObjectCommand import (no rename)
        let get_object = s3_sublibrary.imports.iter()
            .find(|i| i.original_name == "GetObjectCommand")
            .expect("Should find GetObjectCommand import");
        assert_eq!(get_object.local_name, "GetObjectCommand");
        assert!(!get_object.is_renamed);

        // Test client-dynamodb sublibrary  
        let dynamo_sublibrary = imports.iter().find(|s| s.sublibrary == "client-dynamodb")
            .expect("Should find client-dynamodb sublibrary");
        assert_eq!(dynamo_sublibrary.imports.len(), 2, "client-dynamodb should have 2 imports");
        
        // Verify DynamoDBClient import (with rename)
        let dynamo_client = dynamo_sublibrary.imports.iter()
            .find(|i| i.original_name == "DynamoDBClient")
            .expect("Should find DynamoDBClient import");
        assert_eq!(dynamo_client.local_name, "DynamoDB");
        assert!(dynamo_client.is_renamed);

        // Test lib-dynamodb sublibrary (paginate functions)
        let lib_dynamo_sublibrary = imports.iter().find(|s| s.sublibrary == "lib-dynamodb")
            .expect("Should find lib-dynamodb sublibrary");
        assert_eq!(lib_dynamo_sublibrary.imports.len(), 2, "lib-dynamodb should have 2 imports");
        
        // Verify paginateScan rename
        let paginate_scan = lib_dynamo_sublibrary.imports.iter()
            .find(|i| i.original_name == "paginateScan")
            .expect("Should find paginateScan import");
        assert_eq!(paginate_scan.local_name, "PaginateScanRenamed");
        assert!(paginate_scan.is_renamed);

        // === VERIFY COMMONJS REQUIRES ===
        
        // Test client-lambda require
        let lambda_sublibrary = requires.iter().find(|s| s.sublibrary == "client-lambda")
            .expect("Should find client-lambda sublibrary");
        assert_eq!(lambda_sublibrary.imports.len(), 2, "client-lambda should have 2 imports");
        
        let lambda_client = lambda_sublibrary.imports.iter()
            .find(|i| i.original_name == "LambdaClient")
            .expect("Should find LambdaClient require");
        assert_eq!(lambda_client.local_name, "LambdaClient");
        assert!(!lambda_client.is_renamed);

        // Test client-ses require
        let ses_sublibrary = requires.iter().find(|s| s.sublibrary == "client-ses")
            .expect("Should find client-ses sublibrary");
        assert_eq!(ses_sublibrary.imports.len(), 1, "client-ses should have 1 import");

        // === VERIFY NAME MAPPINGS ===
        
        // Test renamed import mappings
        assert_eq!(s3_sublibrary.name_mappings.get("PutObject"), 
                   Some(&"PutObjectCommand".to_string()), 
                   "Should map local name to original name");
        assert_eq!(dynamo_sublibrary.name_mappings.get("DynamoDB"), 
                   Some(&"DynamoDBClient".to_string()),
                   "Should map renamed client correctly");
        assert_eq!(lib_dynamo_sublibrary.name_mappings.get("PaginateScanRenamed"), 
                   Some(&"paginateScan".to_string()),
                   "Should map renamed paginate function correctly");

        // Test non-renamed mappings
        assert_eq!(s3_sublibrary.name_mappings.get("S3Client"), 
                   Some(&"S3Client".to_string()),
                   "Non-renamed imports should map to themselves");

        // Comprehensive test validates import/require parsing functionality
        // Type extraction and classification methods were removed during cleanup

        println!("✅ Comprehensive import/require scanning test passed!");
        println!("   📦 ES6 Imports: {} sublibraries", imports.len());
        println!("   📦 CommonJS Requires: {} sublibraries", requires.len());
    }

    #[test]
    fn test_position_heuristics_command_instantiation() {
        // Test Command constructor position finding
        let source_with_usage = r#"
import { CreateBucketCommand, PutObjectCommand as PutObject } from "@aws-sdk/client-s3";

const client = new S3Client({ region: "us-east-1" });

async function createBucket() {
  const command = new CreateBucketCommand({ Bucket: "test-bucket" });
  const result = await client.send(command);
}

async function uploadFile() {
  const uploadCommand = new PutObject({ 
    Bucket: "test-bucket", 
    Key: "file.txt", 
    Body: "content" 
  });
  await client.send(uploadCommand);
}
        "#;

        let ast = JavaScript.ast_grep(source_with_usage);
        let scanner = ASTScanner::new(ast, JavaScript.into());
        
        // Should find CreateBucketCommand instantiation at line ~6
        let create_bucket_pos = scanner.find_command_instantiation_with_args("CreateBucketCommand");
        assert!(create_bucket_pos.is_some(), "Should find CreateBucketCommand instantiation");

        // Should find PutObject instantiation (renamed) at line ~11
        let put_object_pos = scanner.find_command_instantiation_with_args("PutObject");
        assert!(put_object_pos.is_some(), "Should find PutObject instantiation");

        // Should return None for command that wasn't used
        let missing_command_pos = scanner.find_command_instantiation_with_args("DeleteBucketCommand");
        assert!(missing_command_pos.is_none(), "Should return None for unused command");

        println!("✅ Command instantiation position heuristics working correctly");
    }

    #[test]
    fn test_position_heuristics_paginate_functions() {
        // Test paginate function call position finding
        let source_with_usage = r#"
import { paginateQuery, paginateListTables as PaginateList } from "@aws-sdk/lib-dynamodb";

const client = new DynamoDBClient({ region: "us-east-1" });

async function queryData() {
  const paginator = paginateQuery(paginatorConfig, params);
  for await (const page of paginator) {
    console.log(page.Items);
  }
}

async function listAllTables() {
  const listPaginator = PaginateList(config, {});
  for await (const page of listPaginator) {
    console.log(page.TableNames);
  }
}
        "#;

        let ast = JavaScript.ast_grep(source_with_usage);
        let scanner = ASTScanner::new(ast, JavaScript.into());
        
        // Should find paginateQuery call at line ~7
        let paginate_query = scanner.find_paginate_function_with_args("paginateQuery");
        assert!(paginate_query.is_some(), "Should find paginateQuery call");

        // Should find PaginateList call (renamed) at line ~14
        let paginate_list = scanner.find_paginate_function_with_args("PaginateList");
        assert!(paginate_list.is_some(), "Should find PaginateList call");

        // Should return None for function that wasn't called
        let missing_function_pos = scanner.find_paginate_function_with_args("paginateScan");
        assert!(missing_function_pos.is_none(), "Should return None for unused function");

        println!("✅ Paginate function position heuristics working correctly");
    }

    #[test]
    fn test_position_heuristics_command_input_typescript() {
        // Test CommandInput type usage position finding (TypeScript-specific)
        let typescript_source = r#"
import { QueryCommandInput, ListTablesInput } from "@aws-sdk/lib-dynamodb";

interface User {
  id: string;
  name: string;
}

const queryParams: QueryCommandInput = {
  TableName: 'Users',
  KeyConditionExpression: 'pk = :pk'
};

function createListParams(): ListTablesInput {
  const params: ListTablesInput = {
    Limit: 10
  };
  return params;
}
        "#;

        let ast = TypeScript.ast_grep(typescript_source);
        let scanner = ASTScanner::new(ast, TypeScript.into());
        
        // Should find QueryCommandInput usage at line ~8
        let query_input_pos = scanner.find_command_input_usage_position("QueryCommandInput");
        assert!(query_input_pos.is_some(), "Should find QueryCommandInput usage");
        assert!(query_input_pos.unwrap() > 7 && query_input_pos.unwrap() < 11,
                "QueryCommandInput should be around line 8-9");

        // Should find ListTablesInput usage at line ~15
        let list_input_pos = scanner.find_command_input_usage_position("ListTablesInput");
        assert!(list_input_pos.is_some(), "Should find ListTablesInput usage");
        assert!(list_input_pos.unwrap() > 14 && list_input_pos.unwrap() < 18,
                "ListTablesInput should be around line 15-16");

        // Should return None for type that wasn't used
        let missing_type_pos = scanner.find_command_input_usage_position("PutItemInput");
        assert!(missing_type_pos.is_none(), "Should return None for unused type");

        println!("✅ CommandInput type usage position heuristics working correctly");
    }

    #[test]
    fn test_position_heuristics_javascript_fallback() {
        // Test that JavaScript scanner can find command instantiation
        let javascript_source = r#"
const { CreateBucketCommand } = require("@aws-sdk/client-s3");

const command = new CreateBucketCommand({ Bucket: "test" });
        "#;

        let ast = JavaScript.ast_grep(javascript_source);
        let scanner = ASTScanner::new(ast, JavaScript.into());
        
        // JavaScript should find command instantiation
        let command_pos = scanner.find_command_instantiation_with_args("CreateBucketCommand");
        assert!(command_pos.is_some(), "Should find command instantiation in JavaScript");

        // JavaScript should return None for TypeScript-specific CommandInput usage
        let type_pos = scanner.find_command_input_usage_position("QueryCommandInput");
        assert!(type_pos.is_none(), "Should return None for CommandInput in JavaScript");

        println!("✅ JavaScript fallback behavior working correctly");
    }

    #[test]
    fn test_comprehensive_require_patterns() {
        // Test all supported require variations (const, let, var)
        let source_with_mixed_requires = r#"
// Test const destructuring (original pattern)
const { S3Client, CreateBucketCommand } = require("@aws-sdk/client-s3");

// Test let destructuring (new pattern)
let { DynamoDBClient, QueryCommand as Query } = require("@aws-sdk/client-dynamodb");

// Test var destructuring (legacy pattern)
var { LambdaClient } = require("@aws-sdk/client-lambda");

// Test default imports
const s3Sdk = require("@aws-sdk/client-s3");
let dynamoSdk = require("@aws-sdk/lib-dynamodb");
var ec2Sdk = require("@aws-sdk/client-ec2");
        "#;

        let ast = JavaScript.ast_grep(source_with_mixed_requires);
        let mut scanner = ASTScanner::new(ast, JavaScript.into());
        let (imports, requires) = scanner.scan_all_aws_imports().unwrap();

        // === VERIFY COUNTS ===
        assert_eq!(imports.len(), 0, "Should find 0 ES6 imports");
        
        // Should find requires from all three patterns: const, let, var
        // But may be fewer than 6 due to deduplication by sublibrary
        assert!(requires.len() >= 3, "Should find at least 3 require sublibraries (client-s3, client-dynamodb, client-lambda)");
        assert!(requires.len() <= 8, "Should find at most 8 require sublibraries");

        // === VERIFY SPECIFIC PATTERNS ===
        
        // Should find client-s3 from const destructuring
        let s3_sublibrary = requires.iter().find(|s| s.sublibrary == "client-s3");
        assert!(s3_sublibrary.is_some(), "Should find client-s3 from const require");
        
        // Should find client-dynamodb from let destructuring  
        let dynamo_sublibrary = requires.iter().find(|s| s.sublibrary == "client-dynamodb");
        assert!(dynamo_sublibrary.is_some(), "Should find client-dynamodb from let require");
        
        // Should find client-lambda from var destructuring
        let lambda_sublibrary = requires.iter().find(|s| s.sublibrary == "client-lambda");
        assert!(lambda_sublibrary.is_some(), "Should find client-lambda from var require");

        // === VERIFY IMPORT PARSING ===
        if let Some(s3_sub) = s3_sublibrary {
            // Should find both S3Client and CreateBucketCommand from const destructuring
            assert!(s3_sub.imports.len() >= 2, "Should find at least 2 imports from const destructuring");
            
            let s3_client = s3_sub.imports.iter().find(|i| i.original_name == "S3Client");
            assert!(s3_client.is_some(), "Should find S3Client from const require");
            
            let create_bucket = s3_sub.imports.iter().find(|i| i.original_name == "CreateBucketCommand");
            assert!(create_bucket.is_some(), "Should find CreateBucketCommand from const require");
        }

        if let Some(dynamo_sub) = dynamo_sublibrary {
            // Should find DynamoDBClient and renamed QueryCommand from let destructuring
            assert!(dynamo_sub.imports.len() >= 2, "Should find at least 2 imports from let destructuring");
            
            let dynamo_client = dynamo_sub.imports.iter().find(|i| i.original_name == "DynamoDBClient");
            assert!(dynamo_client.is_some(), "Should find DynamoDBClient from let require");
            
            let query_renamed = dynamo_sub.imports.iter().find(|i| i.original_name == "QueryCommand" && i.local_name == "Query");
            assert!(query_renamed.is_some(), "Should find renamed QueryCommand as Query from let require");
        }

        println!("✅ Comprehensive require pattern test passed!");
        println!("   📦 Found {} require sublibraries covering const/let/var patterns", requires.len());
        
        for sublibrary in &requires {
            println!("   - {} ({} imports)", sublibrary.sublibrary, sublibrary.imports.len());
        }
    }
}
