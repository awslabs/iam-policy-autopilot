use convert_case::{Case, Casing};
use iam_policy_autopilot_policy_generation::{
    api::{extract_sdk_calls, model::ExtractSdkCallsConfig},
    Language, ServiceDiscovery,
};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Test that generates programs with waiters for all services with waiters-2.json files
/// and verifies that iam-policy-autopilot can extract the expected SDK calls.
///
/// This test iterates over [`Language::supported()`] so that adding a new language to the crate
/// automatically causes this test to fail until the new language is wired up here.
#[tokio::test]
async fn test_waiters_extraction() {
    let botocore_data_path = "resources/config/sdks/botocore-data/botocore/data";

    // Discover all services with waiters-2.json files
    let service_waiters = discover_service_waiters(botocore_data_path);

    println!(
        "Found {} services with waiters-2.json files",
        service_waiters.len()
    );

    for service_info in service_waiters {
        for waiter in service_info.waiters.iter() {
            let runner = TestRunner::new(service_info.clone(), waiter.clone());

            // If a language is added to Language::supported() but not handled below, the spawned
            // task panics with a clear message — which is caught by JoinSet and reported.
            for language in Language::supported() {
                match language {
                    Language::Python => runner.test_python().await,
                    Language::Go => runner.test_go().await,
                    Language::JavaScript => runner.test_javascript().await,
                    Language::TypeScript => runner.test_typescript().await,
                    Language::Java => runner.test_java().await,
                    #[allow(unreachable_patterns)]
                    other => panic!(
                        "Language {:?} is listed in Language::all() but has no test \
                         implementation in tests/waiters_extraction.rs. \
                         Please add a test_<language>() method to TestRunner and handle \
                         it in the match arm above.",
                        other
                    ),
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct ServiceWaiterInfo {
    service_name: String,
    waiters: Vec<WaiterOperation>,
    botocore_data_path: PathBuf,
}

#[derive(Debug, Clone)]
struct WaiterOperation {
    waiter_name: String,
    operation: String,
}

#[derive(Debug, Clone)]
struct OperationParameter {
    name: String,
    required: bool,
    shape: String,
}

/// AWS Service definition structure for deserializing service-2.json
#[derive(Debug, serde::Deserialize)]
struct ServiceDefinition {
    operations: std::collections::HashMap<String, Operation>,
    shapes: std::collections::HashMap<String, Shape>,
}

#[derive(Debug, serde::Deserialize)]
struct Operation {
    input: Option<InputShape>,
}

#[derive(Debug, serde::Deserialize)]
struct InputShape {
    shape: String,
}

#[derive(Debug, serde::Deserialize)]
struct Shape {
    #[serde(default)]
    required: Vec<String>,
    members: Option<std::collections::HashMap<String, ShapeMember>>,
}

#[derive(Debug, serde::Deserialize)]
struct ShapeMember {
    shape: String,
}

/// AWS Waiters definition structure for deserializing waiters-2.json
#[derive(Debug, serde::Deserialize)]
struct WaitersDefinition {
    waiters: std::collections::HashMap<String, WaiterDefinition>,
}

#[derive(Debug, serde::Deserialize)]
struct WaiterDefinition {
    operation: String,
}

/// TestRunner encapsulates service information and waiter details,
/// providing code generators for different programming languages
#[derive(Debug, Clone)]
struct TestRunner {
    service_info: ServiceWaiterInfo,
    waiter: WaiterOperation,
    parameters: Vec<OperationParameter>,
}

impl TestRunner {
    /// Create a new TestProgram instance
    fn new(service_info: ServiceWaiterInfo, waiter: WaiterOperation) -> Self {
        // Parse operation parameters once and store them
        let parameters = parse_operation_parameters(&service_info, &waiter.operation);

        Self {
            service_info,
            waiter,
            parameters,
        }
    }

    /// Generate Python code for the waiter test using original pattern (get_waiter only)
    fn generate_python_code_pattern0(&self) -> String {
        let service_name = &self.service_info.service_name;
        let operation = &self.waiter.operation;
        let waiter_name = &self.waiter.waiter_name;

        // Convert operation to snake_case for Python
        let waiter_snake = aws_python_case_conversion(waiter_name);

        format!(
            r#"import boto3

def test_waiter_operation():
    """
    Test for service: {}
    Waiter: {}
    Operation: {}
    Pattern: Original - $CLIENT.get_waiter($NAME)
    """
    client = boto3.client('{}')
    waiter = client.get_waiter('{}')
"#,
            service_name, waiter_name, operation, service_name, waiter_snake,
        )
    }

    /// Generate Python code for the waiter test using separate waiter creation and wait call
    fn generate_python_code_pattern1(&self) -> String {
        let service_name = &self.service_info.service_name;
        let operation = &self.waiter.operation;
        let waiter_name = &self.waiter.waiter_name;

        // Convert operation to snake_case for Python
        let waiter_snake = aws_python_case_conversion(waiter_name);

        // Generate parameter assignments for the wait call using stored parameters
        let mut param_assignments = Vec::new();
        for param in &self.parameters {
            if param.required {
                let value = generate_python_mock_value_for_shape(&param.shape);
                param_assignments.push(format!("        {}={}", param.name, value));
            }
        }

        let wait_args = if param_assignments.is_empty() {
            String::new()
        } else {
            format!(",\n{}", param_assignments.join(",\n"))
        };

        format!(
            r#"import boto3

def test_waiter_operation():
    """
    Test for service: {}
    Waiter: {}
    Operation: {}
    Pattern: $WAITER.wait($$$ARGS)
    """
    client = boto3.client('{}')
    waiter = client.get_waiter('{}')
    waiter.wait({})
"#,
            service_name, waiter_name, operation, service_name, waiter_snake, wait_args
        )
    }

    /// Generate Python code for the waiter test using chained call
    fn generate_python_code_pattern2(&self) -> String {
        let service_name = &self.service_info.service_name;
        let operation = &self.waiter.operation;
        let waiter_name = &self.waiter.waiter_name;

        // Convert operation to snake_case for Python
        let waiter_snake = aws_python_case_conversion(waiter_name);

        // Generate parameter assignments for the wait call using stored parameters
        let mut param_assignments = Vec::new();
        for param in &self.parameters {
            if param.required {
                let value = generate_python_mock_value_for_shape(&param.shape);
                param_assignments.push(format!("        {}={}", param.name, value));
            }
        }

        let wait_args = if param_assignments.is_empty() {
            String::new()
        } else {
            format!("\n{}\n    ", param_assignments.join(",\n"))
        };

        format!(
            r#"import boto3

def test_waiter_operation():
    """
    Test for service: {}
    Waiter: {}
    Operation: {}
    Pattern: $CLIENT.get_waiter($NAME $$$WAITER_ARGS).wait($$$WAIT_ARGS)
    """
    client = boto3.client('{}')
    client.get_waiter('{}').wait({})
"#,
            service_name, waiter_name, operation, service_name, waiter_snake, wait_args
        )
    }

    /// Generate Go code for the waiter test with wait call
    fn generate_go_code_with_wait(&self) -> String {
        let service_name = &self.service_info.service_name;
        let operation = &self.waiter.operation;
        let waiter_name = &self.waiter.waiter_name;

        // Convert service name to appropriate Go package name
        let package_name = service_name.replace("-", "");

        // Generate waiter constructor name (e.g., BucketExists -> NewBucketExistsWaiter)
        let waiter_constructor = format!("New{}Waiter", waiter_name);

        // Generate parameter assignments for the input struct using stored parameters
        let mut param_assignments = Vec::new();
        for param in &self.parameters {
            if param.required {
                let value = generate_go_mock_value_for_shape(&param.shape);
                param_assignments.push(format!("        {}: {},", param.name, value));
            }
        }

        let params_struct = if param_assignments.is_empty() {
            format!("&{}.{}Input{{}}", package_name, operation)
        } else {
            format!(
                "&{}.{}Input{{\n{}\n    }}",
                package_name,
                operation,
                param_assignments.join("\n")
            )
        };

        format!(
            r#"
package main

import (
    "context"
    "fmt"
    "github.com/aws/aws-sdk-go-v2/config"
    "github.com/aws/aws-sdk-go-v2/service/{package_name}"
)

func main() {{
    // Test for service: {service_name}
    // Waiter: {waiter_name}
    // Operation: {operation}
    // Pattern: waiter.Wait()
    
    cfg, err := config.LoadDefaultConfig(context.TODO())
    if err != nil {{
        fmt.Printf("Error loading config: %v\n", err)
        return
    }}
    
    client := {package_name}.NewFromConfig(cfg)
    
    // Use waiter struct instead of calling operation directly
    waiter := {package_name}.{waiter_constructor}(client)
    err = waiter.Wait(context.TODO(), {params_struct})
    if err != nil {{
        fmt.Printf("Waiter {waiter_name} failed: %v\n", err)
    }} else {{
        fmt.Printf("Waiter {waiter_name} completed successfully\n")
    }}
}}
"#,
            service_name = service_name,
            waiter_name = waiter_name,
            operation = operation,
            package_name = package_name,
            waiter_constructor = waiter_constructor,
            params_struct = params_struct
        )
    }

    /// Generate Go code for the waiter test without wait call (just waiter creation)
    fn generate_go_code_without_wait(&self) -> String {
        let service_name = &self.service_info.service_name;
        let operation = &self.waiter.operation;
        let waiter_name = &self.waiter.waiter_name;

        // Convert service name to appropriate Go package name
        let package_name = service_name.replace("-", "");

        // Generate waiter constructor name (e.g., BucketExists -> NewBucketExistsWaiter)
        let waiter_constructor = format!("New{}Waiter", waiter_name);

        format!(
            r#"
package main

import (
    "context"
    "fmt"
    "github.com/aws/aws-sdk-go-v2/config"
    "github.com/aws/aws-sdk-go-v2/service/{package_name}"
)

func main() {{
    // Test for service: {service_name}
    // Waiter: {waiter_name}
    // Operation: {operation}
    // Pattern: waiter creation only
    
    cfg, err := config.LoadDefaultConfig(context.TODO())
    if err != nil {{
        fmt.Printf("Error loading config: %v\n", err)
        return
    }}
    
    client := {package_name}.NewFromConfig(cfg)
    
    // Create waiter but don't call Wait
    waiter := {package_name}.{waiter_constructor}(client)
    fmt.Printf("Created waiter for {waiter_name}\n")
}}
"#,
            service_name = service_name,
            waiter_name = waiter_name,
            operation = operation,
            package_name = package_name,
            waiter_constructor = waiter_constructor
        )
    }

    /// Generate JavaScript code for the waiter test
    fn generate_javascript_code(&self) -> String {
        let service_name = &self.service_info.service_name;
        let operation = &self.waiter.operation;
        let waiter_name = &self.waiter.waiter_name;

        // Convert service name to appropriate JavaScript client name
        let client_class = format!(
            "{}Client",
            service_name.replace("-", " ").to_case(Case::Pascal)
        );

        // Generate waitUntil function name (e.g., BucketExists -> waitUntilBucketExists)
        let wait_until_function = format!("waitUntil{}", waiter_name);

        // Generate parameter object for the waitUntil function using stored parameters
        let mut param_assignments = Vec::new();
        for param in &self.parameters {
            if param.required {
                let value = generate_js_mock_value_for_shape(&param.shape);
                param_assignments.push(format!("    {}: {}", param.name, value));
            }
        }

        let params_object = if param_assignments.is_empty() {
            "{}".to_string()
        } else {
            format!("{{\n{}\n  }}", param_assignments.join(",\n"))
        };

        format!(
            r#"
const {{ {client_class}, {wait_until_function} }} = require("@aws-sdk/client-{service_name}");

async function testWaiterOperation() {{
    /**
     * Test for service: {service_name}
     * Waiter: {waiter_name}
     * Operation: {operation}
     */
    const client = new {client_class}({{}});
    
    try {{
        await {wait_until_function}({{ client, maxWaitTime: 60 }}, {params_object});
        console.log(`Waiter {waiter_name} completed successfully`);
    }} catch (error) {{
        console.log(`Waiter {waiter_name} failed: ${{error.message}}`);
    }}
}}

testWaiterOperation();
"#,
            service_name = service_name,
            waiter_name = waiter_name,
            operation = operation,
            client_class = client_class,
            wait_until_function = wait_until_function,
            params_object = params_object
        )
    }

    /// Generate TypeScript code for the waiter test
    fn generate_typescript_code(&self) -> String {
        let service_name = &self.service_info.service_name;
        let operation = &self.waiter.operation;
        let waiter_name = &self.waiter.waiter_name;

        // Convert service name to appropriate TypeScript client name
        let client_class = format!(
            "{}Client",
            service_name.replace("-", " ").to_case(Case::Pascal)
        );

        // Generate waitUntil function name (e.g., BucketExists -> waitUntilBucketExists)
        let wait_until_function = format!("waitUntil{}", waiter_name);

        // Generate parameter object for the waitUntil function using stored parameters
        let mut param_assignments = Vec::new();
        for param in &self.parameters {
            if param.required {
                let value = generate_js_mock_value_for_shape(&param.shape);
                param_assignments.push(format!("    {}: {}", param.name, value));
            }
        }

        let params_object = if param_assignments.is_empty() {
            "{}".to_string()
        } else {
            format!("{{\n{}\n  }}", param_assignments.join(",\n"))
        };

        format!(
            r#"
import {{ {client_class}, {wait_until_function} }} from "@aws-sdk/client-{service_name}";

async function testWaiterOperation(): Promise<void> {{
    /**
     * Test for service: {service_name}
     * Waiter: {waiter_name}
     * Operation: {operation}
     */
    const client = new {client_class}({{}});
    
    try {{
        await {wait_until_function}({{ client, maxWaitTime: 60 }}, {params_object});
        console.log(`Waiter {waiter_name} completed successfully`);
    }} catch (error) {{
        console.log(`Waiter {waiter_name} failed: ${{(error as Error).message}}`);
    }}
}}

testWaiterOperation();
"#,
            service_name = service_name,
            waiter_name = waiter_name,
            operation = operation,
            client_class = client_class,
            wait_until_function = wait_until_function,
            params_object = params_object
        )
    }

    /// Generate Java code for the waiter test.
    ///
    /// Java SDK v2 waiters are called via `client.waiter().waitUntil<WaiterName>(req -> ...)`.
    /// The extractor strips the `waitUntil` prefix and converts the remainder to camelCase to
    /// produce the waiter type (e.g. `waitUntilTableExists` → `tableExists`).  Our fix then
    /// maps that waiter type to the underlying polling operation in camelCase (e.g.
    /// `describeTable`), which is what the enrichment layer needs to resolve `dynamodb:DescribeTable`.
    fn generate_java_code(&self) -> String {
        let service_name = &self.service_info.service_name;
        let operation = &self.waiter.operation;
        let waiter_name = &self.waiter.waiter_name;

        // Derive the Java SDK v2 client/waiter class names from the service name.
        let client_class = format!(
            "{}Client",
            service_name.replace("-", " ").to_case(Case::Pascal)
        );
        let waiter_class = format!(
            "{}Waiter",
            service_name.replace("-", " ").to_case(Case::Pascal)
        );

        // The Java SDK v2 waiter method is `waitUntil<WaiterName>` (PascalCase waiter name).
        let wait_until_method = format!("waitUntil{}", waiter_name);

        // Derive the package name: replace hyphens with nothing.
        let package_name = service_name.replace("-", "");

        format!(
            r#"import software.amazon.awssdk.services.{package_name}.{client_class};
import software.amazon.awssdk.services.{package_name}.waiters.{waiter_class};

/**
 * Test for service: {service_name}
 * Waiter: {waiter_name}
 * Operation: {operation}
 */
class WaiterTest {{
    void run({client_class} client) {{
        {waiter_class} waiter = client.waiter();
        waiter.{wait_until_method}(r -> r.build());
    }}
}}
"#,
            package_name = package_name,
            client_class = client_class,
            waiter_class = waiter_class,
            service_name = service_name,
            waiter_name = waiter_name,
            operation = operation,
            wait_until_method = wait_until_method,
        )
    }

    /// Test the program for a specific language.
    ///
    /// `expected_operation_name` is the name we expect to see in the extracted `SdkMethodCall`.
    /// It differs by language:
    /// - Python: snake_case of the underlying operation (e.g. `"describe_table"`)
    /// - Go / JS / TS: PascalCase operation name (e.g. `"DescribeTable"`)
    /// - Java: camelCase of the underlying operation (e.g. `"describeTable"`) — this is what
    ///   the waiter matcher emits after our fix
    async fn test_language(
        &self,
        language: &str,
        file_extension: &str,
        code: String,
        expected_operation_name: &str,
    ) {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let file_path = temp_dir
            .path()
            .join(format!("test_waiter.{}", file_extension));

        std::fs::write(&file_path, code).expect(&format!("Failed to write {} file", language));

        // Extract SDK calls
        let config = ExtractSdkCallsConfig {
            source_files: vec![file_path.clone()],
            language: Some(language.to_lowercase()),
            service_hints: None,
        };

        match extract_sdk_calls(&config).await {
            Ok(response) => {
                let found_operation = response.methods.iter().any(|call| {
                    call.name == expected_operation_name
                        && call
                            .possible_services
                            .contains(&self.service_info.service_name)
                });

                assert!(
                    found_operation,
                    "Expected to find operation '{}' for service '{}' in {} program, but got: {:?}",
                    expected_operation_name,
                    self.service_info.service_name,
                    language,
                    response.methods
                );

                println!(
                    "    ✓ {}: Found {} SDK calls",
                    language,
                    response.methods.len()
                );
            }
            Err(e) => {
                panic!(
                    "Failed to extract SDK calls from {} program: {}",
                    language, e
                );
            }
        }
    }

    /// Test Python program with all three waiter patterns.
    ///
    /// Python extracts the underlying operation in snake_case (e.g. `"describe_table"`).
    async fn test_python(&self) {
        let expected =
            ServiceDiscovery::operation_to_method_name(&self.waiter.operation, Language::Python);

        // Test pattern 0: $CLIENT.get_waiter($NAME) (no wait call)
        let code_pattern0 = self.generate_python_code_pattern0();
        self.test_language("Python", "py", code_pattern0, &expected)
            .await;

        // Test pattern 1: $WAITER.wait($$$ARGS)
        let code_pattern1 = self.generate_python_code_pattern1();
        self.test_language("Python", "py", code_pattern1, &expected)
            .await;

        // Test pattern 2: $CLIENT.get_waiter($NAME $$$WAITER_ARGS).wait($$$WAIT_ARGS)
        let code_pattern2 = self.generate_python_code_pattern2();
        self.test_language("Python", "py", code_pattern2, &expected)
            .await;
    }

    /// Test Go program with both patterns.
    ///
    /// Go extracts the underlying operation in PascalCase (e.g. `"DescribeTable"`).
    async fn test_go(&self) {
        let expected = &self.waiter.operation;

        // Test pattern with wait call
        let code_with_wait = self.generate_go_code_with_wait();
        self.test_language("Go", "go", code_with_wait, expected)
            .await;

        // Test pattern without wait call (just waiter creation)
        let code_without_wait = self.generate_go_code_without_wait();
        self.test_language("Go", "go", code_without_wait, expected)
            .await;
    }

    /// Test JavaScript program.
    ///
    /// JavaScript extracts the underlying operation in PascalCase (e.g. `"DescribeTable"`).
    async fn test_javascript(&self) {
        let code = self.generate_javascript_code();
        self.test_language("JavaScript", "js", code, &self.waiter.operation)
            .await;
    }

    /// Test TypeScript program.
    ///
    /// TypeScript extracts the underlying operation in PascalCase (e.g. `"DescribeTable"`).
    async fn test_typescript(&self) {
        let code = self.generate_typescript_code();
        self.test_language("TypeScript", "ts", code, &self.waiter.operation)
            .await;
    }

    /// Test Java program.
    ///
    /// Java SDK v2 waiters are extracted as the underlying polling operation in camelCase
    /// (e.g. `"describeTable"` for `waitUntilTableExists`).  This is the result of our fix
    /// in `src/extraction/java/matchers/waiter.rs` which maps the waiter type to the
    /// underlying operation name via `ServiceModelIndex::waiter_lookup`.
    async fn test_java(&self) {
        let code = self.generate_java_code();
        // Java emits the underlying operation in camelCase (e.g. "describeTable")
        let expected = self.waiter.operation.to_case(Case::Camel);
        self.test_language("Java", "java", code, &expected).await;
    }
}

fn discover_service_waiters(botocore_data_path: &str) -> Vec<ServiceWaiterInfo> {
    let mut service_waiters = Vec::new();

    if let Ok(entries) = fs::read_dir(botocore_data_path) {
        for entry in entries.flatten() {
            if entry.file_type().map_or(false, |ft| ft.is_dir()) {
                let service_name = entry.file_name().to_string_lossy().to_string();

                // Look for version directories and find the latest one
                if let Ok(version_entries) = fs::read_dir(entry.path()) {
                    let mut versions: Vec<_> = version_entries
                        .flatten()
                        .filter(|version_entry| {
                            version_entry.file_type().map_or(false, |ft| ft.is_dir())
                        })
                        .map(|version_entry| {
                            let version = version_entry.file_name().to_string_lossy().to_string();
                            (version, version_entry.path())
                        })
                        .collect();

                    // Sort versions to get the latest one (lexicographically, which works for date-based versions)
                    versions.sort_by(|a, b| b.0.cmp(&a.0));

                    // Only check the latest version
                    if let Some((_latest_version, latest_path)) = versions.first() {
                        let waiters_file = latest_path.join("waiters-2.json");

                        if waiters_file.exists() {
                            if let Ok(waiters) = parse_waiters_file(&waiters_file) {
                                service_waiters.push(ServiceWaiterInfo {
                                    service_name: service_name.clone(),
                                    waiters,
                                    botocore_data_path: latest_path.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    service_waiters
}

fn parse_waiters_file(
    waiters_file: &Path,
) -> Result<Vec<WaiterOperation>, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(waiters_file)?;
    let waiters_def: WaitersDefinition = serde_json::from_str(&content)?;

    let waiters: Vec<WaiterOperation> = waiters_def
        .waiters
        .into_iter()
        .map(|(waiter_name, waiter_config)| WaiterOperation {
            waiter_name,
            operation: waiter_config.operation,
        })
        .collect();

    Ok(waiters)
}

fn parse_operation_parameters(
    service_info: &ServiceWaiterInfo,
    operation: &str,
) -> Vec<OperationParameter> {
    let service_file = service_info.botocore_data_path.join("service-2.json");

    if !service_file.exists() {
        return Vec::new();
    }

    let Ok(content) = std::fs::read_to_string(&service_file) else {
        return Vec::new();
    };

    let Ok(service_def): Result<ServiceDefinition, _> = serde_json::from_str(&content) else {
        return Vec::new();
    };

    // Find the operation
    let Some(operation_def) = service_def.operations.get(operation) else {
        return Vec::new();
    };

    // Get the input shape name
    let Some(input_shape_ref) = &operation_def.input else {
        return Vec::new();
    };

    // Find the shape definition
    let Some(input_shape) = service_def.shapes.get(&input_shape_ref.shape) else {
        return Vec::new();
    };

    let mut parameters = Vec::new();

    // Get all members
    if let Some(members) = &input_shape.members {
        for (param_name, param_def) in members {
            parameters.push(OperationParameter {
                name: param_name.clone(),
                required: input_shape.required.contains(param_name),
                shape: param_def.shape.clone(),
            });
        }
    }

    parameters
}

fn generate_js_mock_value_for_shape(shape: &str) -> String {
    match shape {
        "BucketName" => "\"test-bucket\"".to_string(),
        "String" => "\"test-value\"".to_string(),
        "Integer" => "123".to_string(),
        "Long" => "123".to_string(),
        "Boolean" => "true".to_string(),
        "Timestamp" => "new Date()".to_string(),
        _ => {
            // For unknown shapes, try to infer from the name
            if shape.contains("Name") || shape.contains("Id") || shape.contains("Key") {
                "\"test-value\"".to_string()
            } else if shape.contains("Number") || shape.contains("Count") || shape.contains("Size")
            {
                "123".to_string()
            } else if shape.contains("Boolean") || shape.contains("Flag") {
                "true".to_string()
            } else {
                "\"test-value\"".to_string()
            }
        }
    }
}

fn generate_go_mock_value_for_shape(shape: &str) -> String {
    match shape {
        "BucketName" => "aws.String(\"test-bucket\")".to_string(),
        "String" => "aws.String(\"test-value\")".to_string(),
        "Integer" => "aws.Int32(123)".to_string(),
        "Long" => "aws.Int64(123)".to_string(),
        "Boolean" => "aws.Bool(true)".to_string(),
        "Timestamp" => "aws.Time(time.Now())".to_string(),
        _ => {
            // For unknown shapes, try to infer from the name
            if shape.contains("Name") || shape.contains("Id") || shape.contains("Key") {
                "aws.String(\"test-value\")".to_string()
            } else if shape.contains("Number") || shape.contains("Count") || shape.contains("Size")
            {
                if shape.contains("Long") {
                    "aws.Int64(123)".to_string()
                } else {
                    "aws.Int32(123)".to_string()
                }
            } else if shape.contains("Boolean") || shape.contains("Flag") {
                "aws.Bool(true)".to_string()
            } else {
                "aws.String(\"test-value\")".to_string()
            }
        }
    }
}

fn generate_python_mock_value_for_shape(shape: &str) -> String {
    match shape {
        "BucketName" => "\"test-bucket\"".to_string(),
        "String" => "\"test-value\"".to_string(),
        "Integer" => "123".to_string(),
        "Long" => "123".to_string(),
        "Boolean" => "True".to_string(),
        "Timestamp" => "\"2023-01-01T00:00:00Z\"".to_string(),
        _ => {
            // For unknown shapes, try to infer from the name
            if shape.contains("Name") || shape.contains("Id") || shape.contains("Key") {
                "\"test-value\"".to_string()
            } else if shape.contains("Number") || shape.contains("Count") || shape.contains("Size")
            {
                "123".to_string()
            } else if shape.contains("Boolean") || shape.contains("Flag") {
                "True".to_string()
            } else {
                "\"test-value\"".to_string()
            }
        }
    }
}

/// Convert AWS operation names to Python method names with special handling for version suffixes
///
/// This function uses convert_case for the base conversion but fixes AWS-specific patterns
/// like "V2", "V3" suffixes that should not have underscores inserted.
///
/// Examples:
/// - "ListObjectsV2" → "list_objects_v2" (not "list_objects_v_2")
/// - "GetObjectV1" → "get_object_v1" (not "get_object_v_1")
/// - "CreateBucket" → "create_bucket" (normal cases unchanged)
fn aws_python_case_conversion(operation_name: &str) -> String {
    // First, apply normal snake_case conversion
    let snake_case = operation_name.to_case(Case::Snake);

    // Fix AWS version suffixes at the end: "_v_N" → "_vN" where N is digits
    // Only replace if "_v_" is followed by digits and is at the end of string
    if snake_case.len() >= 4 && snake_case.ends_with(|c: char| c.is_ascii_digit()) {
        if let Some(v_pos) = snake_case.rfind("_v_") {
            let after_v = &snake_case[v_pos + 3..];
            // Check if everything after "_v_" is digits (ensuring it's a version suffix)
            if after_v.chars().all(|c| c.is_ascii_digit()) {
                let prefix = &snake_case[..v_pos];
                return format!("{prefix}_v{after_v}");
            }
        }
    }

    snake_case
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snake_case_conversion() {
        assert_eq!(aws_python_case_conversion("HeadBucket"), "head_bucket");
        assert_eq!(
            aws_python_case_conversion("DescribeInstances"),
            "describe_instances"
        );
        assert_eq!(
            aws_python_case_conversion("DescribeDBInstances"),
            "describe_db_instances"
        );
    }
}
