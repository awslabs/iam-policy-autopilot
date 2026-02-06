//! Core JavaScript/TypeScript scanning logic for AWS SDK extraction

use crate::extraction::javascript::shared::CommandUsage;
use crate::extraction::javascript::types::{
    ClientInstantiation, ImportInfo, JavaScriptScanResults, MethodCall, SublibraryInfo,
    ValidClientTypes,
};
use crate::extraction::AstWithSourceFile;
use crate::Location;

use ast_grep_core::matcher::Pattern;
use ast_grep_core::{tree_sitter, MatchStrictness, NodeMatch};
use ast_grep_core::{Doc, Node};

use std::collections::HashMap;

fn parse_object_literal(obj_text: &str) -> HashMap<String, String> {
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
            _ => value.to_string(),
        };

        result.insert(key.to_string(), final_value);
    }
}

/// Core AST scanner for JavaScript/TypeScript AWS SDK usage patterns
pub(crate) struct ASTScanner<T>
where
    T: ast_grep_language::LanguageExt,
{
    /// Pre-built AST grep root passed from extractor
    pub(crate) ast_grep: AstWithSourceFile<T>,
    pub(crate) language: ast_grep_language::SupportLang,
}

impl<T> ASTScanner<T>
where
    T: ast_grep_language::LanguageExt,
{
    /// Create a new scanner with pre-built AST from extractor
    pub(crate) fn new(
        ast_grep: AstWithSourceFile<T>,
        language: ast_grep_language::SupportLang,
    ) -> Self {
        Self { ast_grep, language }
    }

    fn parse_and_add_imports(
        &self,
        imports_text: &str,
        sublibrary_info: &mut SublibraryInfo,
        node: &Node<'_, tree_sitter::StrDoc<T>>,
    ) {
        // Handle different import formats
        if imports_text.starts_with('{') && imports_text.ends_with('}') {
            // Destructuring - parse with rename support
            let imports_content = &imports_text[1..imports_text.len() - 1]; // Remove braces

            // Split by comma and parse each import
            for import_item in imports_content.split(',') {
                if let Some(import_info) = self.parse_import_item(import_item, node) {
                    sublibrary_info.add_import(import_info);
                }
            }
        } else {
            // Default import - single identifier
            if let Some(import_info) = self.parse_import_item(imports_text, node) {
                sublibrary_info.add_import(import_info);
            }
        }
    }

    fn parse_import_item(
        &self,
        import_item: &str,
        node: &Node<'_, tree_sitter::StrDoc<T>>,
    ) -> Option<ImportInfo> {
        let import_item = import_item.trim();
        if import_item.is_empty() {
            return None;
        }

        // Check for rename syntax: "OriginalName as LocalName"
        if let Some(as_pos) = import_item.find(" as ") {
            let original_name = import_item[..as_pos].trim().to_string();
            let local_name = import_item[as_pos + 4..].trim().to_string();
            Some(ImportInfo::new(
                original_name,
                local_name,
                import_item,
                Location::from_node(self.ast_grep.source_file.path.clone(), node),
            ))
        } else {
            // No rename - original name is the same as local name
            let import_name = import_item.trim().to_string();
            Some(ImportInfo::new(
                import_name.clone(),
                import_name,
                import_item,
                Location::from_node(self.ast_grep.source_file.path.clone(), node),
            ))
        }
    }

    /// Execute a pattern match against the AST using relaxed strictness to handle inline comments
    fn find_all_matches(
        &self,
        pattern: &str,
    ) -> Result<Vec<NodeMatch<'_, tree_sitter::StrDoc<T>>>, String> {
        let root = self.ast_grep.ast.root();

        // Build pattern with relaxed strictness to handle inline comments
        let pattern_obj =
            Pattern::new(pattern, self.language).with_strictness(MatchStrictness::Relaxed);

        Ok(root.find_all(pattern_obj).collect())
    }

    /// Find Command instantiation and extract its arguments
    /// Returns CommandInstantiationResult with position and parameters
    pub(crate) fn find_command_instantiation_with_args(
        &self,
        command_name: &str,
    ) -> Option<CommandUsage<'_>> {
        use crate::extraction::javascript::argument_extractor::ArgumentExtractor;

        let pattern = format!("new {command_name}($ARGS)");

        if let Ok(matches) = self.find_all_matches(&pattern) {
            if let Some(first_match) = matches.first() {
                let location =
                    Location::from_node(self.ast_grep.source_file.path.clone(), first_match);
                let env = first_match.get_env();

                // Extract arguments from the ARGS node
                // env.get_match returns Option<&Node>, so pass directly
                let args_node = env.get_match("ARGS");
                let parameters = ArgumentExtractor::extract_object_parameters(args_node);

                return Some(CommandUsage::new(first_match.text(), location, parameters));
            }
        }
        None
    }

    /// Find paginate function call and extract operation parameters (2nd argument)
    pub(crate) fn find_paginate_function_with_args(
        &self,
        function_name: &str,
    ) -> Option<CommandUsage<'_>> {
        use crate::extraction::javascript::argument_extractor::ArgumentExtractor;

        // Use explicit two-argument pattern
        let pattern = format!("{function_name}($ARG1, $ARG2)");

        if let Ok(matches) = self.find_all_matches(&pattern) {
            if let Some(first_match) = matches.first() {
                let location =
                    Location::from_node(self.ast_grep.source_file.path.clone(), first_match);
                let env = first_match.get_env();

                // Extract parameters from second argument (ARG2 = operation params)
                let second_arg = env.get_match("ARG2");
                let parameters = ArgumentExtractor::extract_object_parameters(second_arg);

                return Some(CommandUsage::new(first_match.text(), location, parameters));
            }
        }
        None
    }

    /// Find waiter function call and extract operation parameters (2nd argument)
    pub(crate) fn find_waiter_function_with_args(
        &self,
        function_name: &str,
    ) -> Option<CommandUsage<'_>> {
        use crate::extraction::javascript::argument_extractor::ArgumentExtractor;

        // Try patterns with and without await keyword using explicit two-argument pattern
        let patterns = [
            format!("await {function_name}($ARG1, $ARG2)"), // With await
            format!("{function_name}($ARG1, $ARG2)"),       // Without await
        ];

        for pattern in &patterns {
            if let Ok(matches) = self.find_all_matches(pattern) {
                if let Some(first_match) = matches.first() {
                    let location =
                        Location::from_node(self.ast_grep.source_file.path.clone(), first_match);
                    let env = first_match.get_env();

                    // Extract parameters from second argument (ARG2 = operation params)
                    let second_arg = env.get_match("ARG2");
                    let parameters = ArgumentExtractor::extract_object_parameters(second_arg);

                    return Some(CommandUsage::new(first_match.text(), location, parameters));
                }
            }
        }
        None
    }

    /// Find CommandInput type usage position (TypeScript-specific)
    pub(crate) fn find_command_input_usage_position(
        &self,
        type_name: &str,
    ) -> Option<CommandUsage<'_>> {
        // Try multiple patterns for TypeScript type annotations
        let patterns = [
            format!("const $VAR: {type_name} = $VALUE"), // const variable: Type = value
            format!("let $VAR: {type_name} = $VALUE"),   // let variable: Type = value
            format!("$VAR: {type_name} = $VALUE"),       // variable: Type = value
        ];

        for pattern in &patterns {
            if let Ok(matches) = self.find_all_matches(pattern) {
                if let Some(first_match) = matches.first() {
                    let location =
                        Location::from_node(self.ast_grep.source_file.path.clone(), first_match);
                    let expr_text = first_match.text();
                    // TODO: Extract from variable assignments
                    let parameters = vec![];
                    return Some(CommandUsage::new(expr_text, location, parameters));
                }
            }
        }
        None
    }

    /// Scan AWS import/require statements generically
    fn scan_aws_statements(&self, pattern: &str) -> Result<Vec<SublibraryInfo>, String> {
        let mut sublibrary_data: HashMap<String, SublibraryInfo> = HashMap::new();

        let matches = self.find_all_matches(pattern)?;
        self.process_import_matches(matches, &mut sublibrary_data)?;

        Ok(sublibrary_data.into_values().collect())
    }

    /// Generic processing for import/require matches - works for both JavaScript and TypeScript
    fn process_import_matches(
        &self,
        matches: Vec<ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<T>>>,
        sublibrary_data: &mut HashMap<String, SublibraryInfo>,
    ) -> Result<(), String> {
        for node_match in matches {
            let env = node_match.get_env();

            let module_node = env.get_match("MODULE");
            let imports_node = env.get_match("IMPORTS");

            if let (Some(module_node), Some(imports_node)) = (module_node, imports_node) {
                let module_text_cow = module_node.text();
                let module_text = module_text_cow.trim_matches('"').trim_matches('\'');

                // Check if it's an AWS SDK statement
                if let Some(sublibrary) = module_text.strip_prefix("@aws-sdk/") {
                    let sublibrary = sublibrary.to_string();
                    let imports_text = imports_node.text();
                    let imports_text_str = imports_text.as_ref(); // Convert Cow to &str

                    // Initialize sublibrary data if not exists
                    let sublibrary_info = sublibrary_data
                        .entry(sublibrary.clone())
                        .or_insert_with(|| SublibraryInfo::new(sublibrary));

                    self.parse_and_add_imports(
                        imports_text_str,
                        sublibrary_info,
                        node_match.get_node(),
                    );
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
            "const $IMPORTS = require($MODULE)", // Destructuring: const { S3Client } = require(...)
            "let $IMPORTS = require($MODULE)",   // Destructuring: let { S3Client } = require(...)
            "var $IMPORTS = require($MODULE)", // Destructuring: var { S3Client } = require(...) [legacy]
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
        for source_data in &[imports, requires] {
            for sublibrary_info in source_data {
                for import_info in &sublibrary_info.imports {
                    let original_name = &import_info.original_name;
                    let local_name = &import_info.local_name;

                    // Check if it's a client type (starts with uppercase, doesn't end with Command/CommandInput)
                    if original_name.chars().next().is_some_and(char::is_uppercase)
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

        Ok(ValidClientTypes::new(
            client_types,
            name_mappings,
            sublibrary_mappings,
        ))
    }

    /// Scan for AWS client instantiations
    pub(crate) fn scan_client_instantiations(
        &mut self,
    ) -> Result<Vec<ClientInstantiation>, String> {
        // Patterns to match client instantiations
        const PATTERNS: &[&str] = &[
            "const $VAR = new $CLIENT($ARGS)",
            "let $VAR = new $CLIENT($ARGS)",
        ];

        let client_info = self.get_valid_client_types()?;

        if client_info.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

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
        matches: Vec<NodeMatch<U>>,
        valid_client_types: &[String],
        client_name_mappings: &HashMap<String, String>,
        client_sublibrary_mappings: &HashMap<String, String>,
        results: &mut Vec<ClientInstantiation>,
    ) -> Result<(), String>
    where
        U: Doc + std::clone::Clone,
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
    fn process_method_call_matches(
        &self,
        matches: Vec<ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<T>>>,
        client_info_map: &HashMap<String, (String, String, String)>,
        results: &mut Vec<MethodCall>,
    ) -> Result<(), String> {
        for node_match in matches {
            let env = node_match.get_env();

            let var_node = env.get_match("VAR");
            let method_node = env.get_match("METHOD");
            let args_node = env.get_match("ARGS");

            if let (Some(var_node), Some(method_node)) = (var_node, method_node) {
                let variable_name = var_node.text().to_string();
                let method_name = method_node.text().to_string();

                // Check if it's a known client variable
                if let Some((client_type, original_client_type, client_sublibrary)) =
                    client_info_map.get(&variable_name)
                {
                    // Extract arguments
                    let arguments = if let Some(args_node) = args_node {
                        let args_text = args_node.text();
                        parse_object_literal(args_text.as_ref())
                    } else {
                        HashMap::new()
                    };

                    results.push(MethodCall {
                        client_variable: variable_name,
                        client_type: client_type.clone(),
                        original_client_type: original_client_type.clone(),
                        client_sublibrary: client_sublibrary.clone(),
                        expr: node_match.text().to_string(),
                        method_name,
                        arguments,
                        location: Location::from_node(
                            self.ast_grep.source_file.path.clone(),
                            node_match.get_node(),
                        ),
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

        // Single pattern to match method calls (covers both awaited and non-awaited)
        let matches = self.find_all_matches("$VAR.$METHOD($ARGS)")?;
        self.process_method_call_matches(matches, &client_info_map, &mut results)?;

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
    use std::path::PathBuf;

    use crate::SourceFile;

    use super::*;
    use ast_grep_language::{JavaScript, TypeScript};
    use tree_sitter::LanguageExt;

    #[test]
    fn test_parse_object_literal() {
        let result = parse_object_literal("{region: 'us-east-1', timeout: 5000}");
        assert_eq!(result.get("region"), Some(&"us-east-1".to_string()));
        assert_eq!(result.get("timeout"), Some(&"5000".to_string()));

        // Test empty object
        let result = parse_object_literal("{}");
        assert!(result.is_empty());
    }

    fn create_js_ast(source_code: &str) -> AstWithSourceFile<JavaScript> {
        let source_file = SourceFile::with_language(
            PathBuf::new(),
            source_code.to_string(),
            crate::Language::JavaScript,
        );
        let ast_grep = JavaScript.ast_grep(&source_file.content);
        AstWithSourceFile::new(ast_grep, source_file.clone())
    }

    #[test]
    fn test_parse_import_item() {
        // Test regular import
        let source = r#"import S3Client from "@aws-sdk/client-s3""#;
        let ast = create_js_ast(source);
        let mut scanner = ASTScanner::new(ast, JavaScript.into());

        let (imports, _requires) = scanner.scan_all_aws_imports().unwrap();
        assert_eq!(
            ImportInfo::new(
                "S3Client".to_string(),
                "S3Client".to_string(),
                "S3Client",
                Location::new(PathBuf::new(), (1, 1), (1, 42)),
            ),
            imports[0].imports[0]
        );

        // Test renamed import
        let source = r#"import { S3Client as MyS3Client } from "@aws-sdk/client-s3";"#;
        let ast = create_js_ast(source);
        let mut scanner = ASTScanner::new(ast, JavaScript.into());

        let (imports, _requires) = scanner.scan_all_aws_imports().unwrap();
        assert_eq!(
            ImportInfo::new(
                "S3Client".to_string(),
                "MyS3Client".to_string(),
                "S3Client as MyS3Client",
                Location::new(PathBuf::new(), (1, 1), (1, 61)),
            ),
            imports[0].imports[0]
        );
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

        let ast = create_js_ast(source);
        let mut scanner = ASTScanner::new(ast, JavaScript.into());
        let (imports, requires) = scanner.scan_all_aws_imports().unwrap();

        // === VERIFY BASIC COUNTS ===
        assert_eq!(imports.len(), 3, "Should find 3 ES6 import sublibraries");
        assert!(
            requires.len() >= 2,
            "Should find at least 2 CommonJS require sublibraries"
        );

        // === VERIFY ES6 IMPORTS ===

        // Test client-s3 sublibrary
        let s3_sublibrary = imports
            .iter()
            .find(|s| s.sublibrary == "client-s3")
            .expect("Should find client-s3 sublibrary");
        assert_eq!(
            s3_sublibrary.imports.len(),
            3,
            "client-s3 should have 3 imports"
        );

        // Verify S3Client import (no rename)
        let s3_client = s3_sublibrary
            .imports
            .iter()
            .find(|i| i.original_name == "S3Client")
            .expect("Should find S3Client import");
        assert_eq!(s3_client.local_name, "S3Client");
        assert!(!s3_client.is_renamed);

        // Verify PutObjectCommand import (with rename)
        let put_object = s3_sublibrary
            .imports
            .iter()
            .find(|i| i.original_name == "PutObjectCommand")
            .expect("Should find PutObjectCommand import");
        assert_eq!(put_object.local_name, "PutObject");
        assert!(put_object.is_renamed);

        // Verify GetObjectCommand import (no rename)
        let get_object = s3_sublibrary
            .imports
            .iter()
            .find(|i| i.original_name == "GetObjectCommand")
            .expect("Should find GetObjectCommand import");
        assert_eq!(get_object.local_name, "GetObjectCommand");
        assert!(!get_object.is_renamed);

        // Test client-dynamodb sublibrary
        let dynamo_sublibrary = imports
            .iter()
            .find(|s| s.sublibrary == "client-dynamodb")
            .expect("Should find client-dynamodb sublibrary");
        assert_eq!(
            dynamo_sublibrary.imports.len(),
            2,
            "client-dynamodb should have 2 imports"
        );

        // Verify DynamoDBClient import (with rename)
        let dynamo_client = dynamo_sublibrary
            .imports
            .iter()
            .find(|i| i.original_name == "DynamoDBClient")
            .expect("Should find DynamoDBClient import");
        assert_eq!(dynamo_client.local_name, "DynamoDB");
        assert!(dynamo_client.is_renamed);

        // Test lib-dynamodb sublibrary (paginate functions)
        let lib_dynamo_sublibrary = imports
            .iter()
            .find(|s| s.sublibrary == "lib-dynamodb")
            .expect("Should find lib-dynamodb sublibrary");
        assert_eq!(
            lib_dynamo_sublibrary.imports.len(),
            2,
            "lib-dynamodb should have 2 imports"
        );

        // Verify paginateScan rename
        let paginate_scan = lib_dynamo_sublibrary
            .imports
            .iter()
            .find(|i| i.original_name == "paginateScan")
            .expect("Should find paginateScan import");
        assert_eq!(paginate_scan.local_name, "PaginateScanRenamed");
        assert!(paginate_scan.is_renamed);

        // === VERIFY COMMONJS REQUIRES ===

        // Test client-lambda require
        let lambda_sublibrary = requires
            .iter()
            .find(|s| s.sublibrary == "client-lambda")
            .expect("Should find client-lambda sublibrary");
        assert_eq!(
            lambda_sublibrary.imports.len(),
            2,
            "client-lambda should have 2 imports"
        );

        let lambda_client = lambda_sublibrary
            .imports
            .iter()
            .find(|i| i.original_name == "LambdaClient")
            .expect("Should find LambdaClient require");
        assert_eq!(lambda_client.local_name, "LambdaClient");
        assert!(!lambda_client.is_renamed);

        // Test client-ses require
        let ses_sublibrary = requires
            .iter()
            .find(|s| s.sublibrary == "client-ses")
            .expect("Should find client-ses sublibrary");
        assert_eq!(
            ses_sublibrary.imports.len(),
            1,
            "client-ses should have 1 import"
        );

        // === VERIFY NAME MAPPINGS ===

        // Test renamed import mappings
        assert_eq!(
            s3_sublibrary.name_mappings.get("PutObject"),
            Some(&"PutObjectCommand".to_string()),
            "Should map local name to original name"
        );
        assert_eq!(
            dynamo_sublibrary.name_mappings.get("DynamoDB"),
            Some(&"DynamoDBClient".to_string()),
            "Should map renamed client correctly"
        );
        assert_eq!(
            lib_dynamo_sublibrary
                .name_mappings
                .get("PaginateScanRenamed"),
            Some(&"paginateScan".to_string()),
            "Should map renamed paginate function correctly"
        );

        // Test non-renamed mappings
        assert_eq!(
            s3_sublibrary.name_mappings.get("S3Client"),
            Some(&"S3Client".to_string()),
            "Non-renamed imports should map to themselves"
        );

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

        let ast = create_js_ast(source_with_usage);
        let scanner = ASTScanner::new(ast, JavaScript.into());

        // Should find CreateBucketCommand instantiation at line ~6
        let create_bucket_pos = scanner.find_command_instantiation_with_args("CreateBucketCommand");
        assert!(
            create_bucket_pos.is_some(),
            "Should find CreateBucketCommand instantiation"
        );

        // Should find PutObject instantiation (renamed) at line ~11
        let put_object_pos = scanner.find_command_instantiation_with_args("PutObject");
        assert!(
            put_object_pos.is_some(),
            "Should find PutObject instantiation"
        );

        // Should return None for command that wasn't used
        let missing_command_pos =
            scanner.find_command_instantiation_with_args("DeleteBucketCommand");
        assert!(
            missing_command_pos.is_none(),
            "Should return None for unused command"
        );

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

        let ast = create_js_ast(source_with_usage);
        let scanner = ASTScanner::new(ast, JavaScript.into());

        // Should find paginateQuery call at line ~7
        let paginate_query = scanner.find_paginate_function_with_args("paginateQuery");
        assert!(paginate_query.is_some(), "Should find paginateQuery call");

        // Should find PaginateList call (renamed) at line ~14
        let paginate_list = scanner.find_paginate_function_with_args("PaginateList");
        assert!(paginate_list.is_some(), "Should find PaginateList call");

        // Should return None for function that wasn't called
        let missing_function_pos = scanner.find_paginate_function_with_args("paginateScan");
        assert!(
            missing_function_pos.is_none(),
            "Should return None for unused function"
        );

        println!("✅ Paginate function position heuristics working correctly");
    }

    fn create_ts_ast(source_code: &str) -> AstWithSourceFile<TypeScript> {
        let source_file = SourceFile::with_language(
            PathBuf::new(),
            source_code.to_string(),
            crate::Language::TypeScript,
        );
        let ast_grep = TypeScript.ast_grep(&source_file.content);
        AstWithSourceFile::new(ast_grep, source_file.clone())
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

        let ast = create_ts_ast(typescript_source);
        let scanner = ASTScanner::new(ast, TypeScript.into());

        if let Some(result) = scanner.find_command_input_usage_position("QueryCommandInput") {
            assert_eq!(
                result.location.start_line(),
                9,
                "QueryCommandInput should be at line 9"
            );
        } else {
            panic!("Should find QueryCommandInput usage");
        }

        if let Some(result) = scanner.find_command_input_usage_position("ListTablesInput") {
            assert_eq!(
                result.location.start_line(),
                15,
                "ListTablesInput should be at line 15"
            );
        } else {
            panic!("Should find ListTablesInput usage");
        }

        // Should return None for type that wasn't used
        let missing_type_pos = scanner.find_command_input_usage_position("PutItemInput");
        assert!(
            missing_type_pos.is_none(),
            "Should return None for unused type"
        );

        println!("✅ CommandInput type usage position heuristics working correctly");
    }

    #[test]
    fn test_position_heuristics_javascript_fallback() {
        // Test that JavaScript scanner can find command instantiation
        let javascript_source = r#"
const { CreateBucketCommand } = require("@aws-sdk/client-s3");

const command = new CreateBucketCommand({ Bucket: "test" });
        "#;

        let ast = create_js_ast(javascript_source);
        let scanner = ASTScanner::new(ast, JavaScript.into());

        // JavaScript should find command instantiation
        let command_pos = scanner.find_command_instantiation_with_args("CreateBucketCommand");
        assert!(
            command_pos.is_some(),
            "Should find command instantiation in JavaScript"
        );

        // JavaScript should return None for TypeScript-specific CommandInput usage
        let type_pos = scanner.find_command_input_usage_position("QueryCommandInput");
        assert!(
            type_pos.is_none(),
            "Should return None for CommandInput in JavaScript"
        );

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

        let ast = create_js_ast(source_with_mixed_requires);
        let mut scanner = ASTScanner::new(ast, JavaScript.into());
        let (imports, requires) = scanner.scan_all_aws_imports().unwrap();

        // === VERIFY COUNTS ===
        assert_eq!(imports.len(), 0, "Should find 0 ES6 imports");

        // Should find requires from all three patterns: const, let, var
        // But may be fewer than 6 due to deduplication by sublibrary
        assert!(requires.len() >= 3, "Should find at least 3 require sublibraries (client-s3, client-dynamodb, client-lambda)");
        assert!(
            requires.len() <= 8,
            "Should find at most 8 require sublibraries"
        );

        // === VERIFY SPECIFIC PATTERNS ===

        // Should find client-s3 from const destructuring
        let s3_sublibrary = requires.iter().find(|s| s.sublibrary == "client-s3");
        assert!(
            s3_sublibrary.is_some(),
            "Should find client-s3 from const require"
        );

        // Should find client-dynamodb from let destructuring
        let dynamo_sublibrary = requires.iter().find(|s| s.sublibrary == "client-dynamodb");
        assert!(
            dynamo_sublibrary.is_some(),
            "Should find client-dynamodb from let require"
        );

        // Should find client-lambda from var destructuring
        let lambda_sublibrary = requires.iter().find(|s| s.sublibrary == "client-lambda");
        assert!(
            lambda_sublibrary.is_some(),
            "Should find client-lambda from var require"
        );

        // === VERIFY IMPORT PARSING ===
        if let Some(s3_sub) = s3_sublibrary {
            // Should find both S3Client and CreateBucketCommand from const destructuring
            assert!(
                s3_sub.imports.len() >= 2,
                "Should find at least 2 imports from const destructuring"
            );

            let s3_client = s3_sub
                .imports
                .iter()
                .find(|i| i.original_name == "S3Client");
            assert!(
                s3_client.is_some(),
                "Should find S3Client from const require"
            );

            let create_bucket = s3_sub
                .imports
                .iter()
                .find(|i| i.original_name == "CreateBucketCommand");
            assert!(
                create_bucket.is_some(),
                "Should find CreateBucketCommand from const require"
            );
        }

        if let Some(dynamo_sub) = dynamo_sublibrary {
            // Should find DynamoDBClient and renamed QueryCommand from let destructuring
            assert!(
                dynamo_sub.imports.len() >= 2,
                "Should find at least 2 imports from let destructuring"
            );

            let dynamo_client = dynamo_sub
                .imports
                .iter()
                .find(|i| i.original_name == "DynamoDBClient");
            assert!(
                dynamo_client.is_some(),
                "Should find DynamoDBClient from let require"
            );

            let query_renamed = dynamo_sub
                .imports
                .iter()
                .find(|i| i.original_name == "QueryCommand" && i.local_name == "Query");
            assert!(
                query_renamed.is_some(),
                "Should find renamed QueryCommand as Query from let require"
            );
        }

        println!("✅ Comprehensive require pattern test passed!");
        println!(
            "   📦 Found {} require sublibraries covering const/let/var patterns",
            requires.len()
        );

        for sublibrary in &requires {
            println!(
                "   - {} ({} imports)",
                sublibrary.sublibrary,
                sublibrary.imports.len()
            );
        }
    }

    #[test]
    fn test_dynamodb_library_expansions() {
        use crate::extraction::javascript::shared::ExtractionUtils;

        // Test DynamoDB lib-dynamodb expansions from JSON configuration
        let typescript_source = r#"
import { DynamoDBClient } from "@aws-sdk/client-dynamodb";
import { 
    PutCommand, 
    GetCommand, 
    paginateQuery, 
    paginateScan 
} from "@aws-sdk/lib-dynamodb";

const client = new DynamoDBClient({ region: "us-west-2" });

async function testOperations() {
    // Test PutCommand expansion (should expand to PutItemCommand -> PutItem operation)
    const putCmd = new PutCommand({
        TableName: "Users",
        Item: { id: "123", name: "John" }
    });
    
    // Test GetCommand expansion (should expand to GetItemCommand -> GetItem operation)
    const getCmd = new GetCommand({
        TableName: "Users",
        Key: { id: "123" }
    });
    
    // Test paginateQuery expansion (should expand to QueryCommand -> Query operation)
    const queryPaginator = paginateQuery(
        { client },
        { TableName: "Users", KeyConditionExpression: "pk = :pk" }
    );
    
    // Test paginateScan expansion (should expand to ScanCommand -> Scan operation)
    const scanPaginator = paginateScan(
        { client },
        { TableName: "Users" }
    );
}
        "#;

        let ast = create_ts_ast(typescript_source);
        let mut scanner = ASTScanner::new(ast, TypeScript.into());
        let scan_results = scanner.scan_all().unwrap();

        // Extract operations using the full pipeline
        let operations =
            ExtractionUtils::extract_operations_from_imports(&scan_results, &mut scanner);

        // Verify we found all 4 expected operations
        assert!(
            operations.len() >= 4,
            "Should find at least 4 operations from DynamoDB lib-dynamodb expansions"
        );

        // Verify PutCommand expanded to PutItem
        let put_item = operations.iter().find(|op| op.name == "PutItem");
        assert!(
            put_item.is_some(),
            "Should find PutItem operation from PutCommand expansion"
        );
        if let Some(op) = put_item {
            assert_eq!(op.possible_services, vec!["dynamodb".to_string()]);
            // Verify parameters were extracted
            assert!(op.metadata.is_some());
            if let Some(metadata) = &op.metadata {
                assert!(
                    !metadata.parameters.is_empty(),
                    "PutItem should have parameters"
                );
            }
        }

        // Verify GetCommand expanded to GetItem
        let get_item = operations.iter().find(|op| op.name == "GetItem");
        assert!(
            get_item.is_some(),
            "Should find GetItem operation from GetCommand expansion"
        );
        if let Some(op) = get_item {
            assert_eq!(op.possible_services, vec!["dynamodb".to_string()]);
            assert!(op.metadata.is_some());
            if let Some(metadata) = &op.metadata {
                assert!(
                    !metadata.parameters.is_empty(),
                    "GetItem should have parameters"
                );
            }
        }

        // Verify paginateQuery expanded to Query
        let query = operations.iter().find(|op| op.name == "Query");
        assert!(
            query.is_some(),
            "Should find Query operation from paginateQuery expansion"
        );
        if let Some(op) = query {
            assert_eq!(op.possible_services, vec!["dynamodb".to_string()]);
            assert!(op.metadata.is_some());
            if let Some(metadata) = &op.metadata {
                assert!(
                    !metadata.parameters.is_empty(),
                    "Query should have parameters from 2nd argument"
                );
            }
        }

        // Verify paginateScan expanded to Scan
        let scan = operations.iter().find(|op| op.name == "Scan");
        assert!(
            scan.is_some(),
            "Should find Scan operation from paginateScan expansion"
        );
        if let Some(op) = scan {
            assert_eq!(op.possible_services, vec!["dynamodb".to_string()]);
            assert!(op.metadata.is_some());
            if let Some(metadata) = &op.metadata {
                assert!(
                    !metadata.parameters.is_empty(),
                    "Scan should have parameters from 2nd argument"
                );
            }
        }

        println!("✅ DynamoDB lib-dynamodb expansion test passed!");
        println!("   ✓ PutCommand → PutItem");
        println!("   ✓ GetCommand → GetItem");
        println!("   ✓ paginateQuery → Query");
        println!("   ✓ paginateScan → Scan");
        println!("   📊 Total operations extracted: {}", operations.len());
    }

    #[test]
    fn test_s3_storage_library_expansions() {
        use crate::extraction::javascript::shared::ExtractionUtils;

        // Test S3 lib-storage Upload expansion from JSON configuration
        let typescript_source = r#"
import { S3Client } from "@aws-sdk/client-s3";
import { Upload } from "@aws-sdk/lib-storage";

const s3Client = new S3Client({ region: "us-west-2" });

async function uploadLargeFile() {
    // Test Upload class expansion (should expand to 6 S3 operations)
    const upload = new Upload({
        client: s3Client,
        params: {
            Bucket: "my-bucket",
            Key: "uploads/large-file.dat",
            Body: "file-content"
        },
        tags: [
            { Key: "Environment", Value: "Production" }
        ],
        queueSize: 4,
        partSize: 5242880
    });

    const result = await upload.done();
    console.log("Upload completed:", result.Location);
}
        "#;

        let ast = create_ts_ast(typescript_source);
        let mut scanner = ASTScanner::new(ast, TypeScript.into());
        let scan_results = scanner.scan_all().unwrap();

        // Extract operations using the full pipeline
        let operations =
            ExtractionUtils::extract_operations_from_imports(&scan_results, &mut scanner);

        // According to js_v3_libraries.json, Upload should expand to 6 commands:
        // 1. PutObjectCommand → PutObject
        // 2. CreateMultipartUploadCommand → CreateMultipartUpload
        // 3. UploadPartCommand → UploadPart
        // 4. CompleteMultipartUploadCommand → CompleteMultipartUpload
        // 5. AbortMultipartUploadCommand → AbortMultipartUpload
        // 6. PutObjectTaggingCommand → PutObjectTagging

        let expected_operations = vec![
            "PutObject",
            "CreateMultipartUpload",
            "UploadPart",
            "CompleteMultipartUpload",
            "AbortMultipartUpload",
            "PutObjectTagging",
        ];

        assert!(
            operations.len() >= 6,
            "Should find at least 6 operations from Upload expansion, found {}",
            operations.len()
        );

        // Verify each expected operation is present
        for expected_op in &expected_operations {
            let op = operations.iter().find(|o| &o.name == expected_op);
            assert!(
                op.is_some(),
                "Should find {} operation from Upload expansion",
                expected_op
            );

            if let Some(op) = op {
                assert_eq!(
                    op.possible_services,
                    vec!["storage".to_string()],
                    "{} should be mapped to 'storage' service",
                    expected_op
                );

                // Verify metadata is present
                assert!(
                    op.metadata.is_some(),
                    "{} should have metadata",
                    expected_op
                );

                // Verify parameters were extracted from Upload constructor
                if let Some(metadata) = &op.metadata {
                    assert!(
                        !metadata.parameters.is_empty(),
                        "{} should have parameters extracted from Upload constructor",
                        expected_op
                    );
                }
            }
        }

        println!("✅ S3 lib-storage Upload expansion test passed!");
        println!("   ✓ Upload → PutObject");
        println!("   ✓ Upload → CreateMultipartUpload");
        println!("   ✓ Upload → UploadPart");
        println!("   ✓ Upload → CompleteMultipartUpload");
        println!("   ✓ Upload → AbortMultipartUpload");
        println!("   ✓ Upload → PutObjectTagging");
        println!("   📊 Total operations extracted: {}", operations.len());
    }
}
