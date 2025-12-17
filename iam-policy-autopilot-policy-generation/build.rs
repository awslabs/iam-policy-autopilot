use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

/// Simplified service definition with fields removed
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SimplifiedServiceDefinition {
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    metadata: ServiceMetadata,
    operations: HashMap<String, SimplifiedOperation>,
    shapes: HashMap<String, SimplifiedShape>,
}

/// Service metadata from AWS service definitions
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceMetadata {
    #[serde(rename = "apiVersion")]
    api_version: String,
    #[serde(rename = "serviceId")]
    service_id: String,
}

/// Simplified operation definition (removed fields)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SimplifiedOperation {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<ShapeReference>,
}

/// Simplified shape definition (removed fields)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SimplifiedShape {
    #[serde(rename = "type")]
    type_name: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    members: HashMap<String, ShapeReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    required: Option<Vec<String>>,
}

/// Shape reference (removed fields)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShapeReference {
    shape: String,
}

fn main() {
    println!("cargo:rerun-if-changed=resources/config/sdks/botocore-data");
    println!("cargo:rerun-if-changed=resources/config/sdks/boto3");
    println!("cargo:rerun-if-env-changed=IAM_POLICY_AUTOPILOT_INTEGRATION_TEST");

    let out_dir = env::var("OUT_DIR").unwrap();
    let simplified_dir = Path::new(&out_dir).join("botocore-data-simplified");
    let boto3_dir = Path::new(&out_dir).join("boto3-data-simplified");

    // Create the simplified directories
    if let Err(e) = fs::create_dir_all(&simplified_dir) {
        panic!("Failed to create botocore simplified directory: {}", e);
    }
    if let Err(e) = fs::create_dir_all(&boto3_dir) {
        panic!("Failed to create boto3 simplified directory: {}", e);
    }

    // Process botocore data
    let botocore_data_path = Path::new("resources/config/sdks/botocore-data/botocore/data");
    if !botocore_data_path.exists() {
        panic!(
            "Required botocore data directory not found at: {}. Please ensure the botocore data \
             is available by running `git submodule init && git submodule update`.",
            botocore_data_path.display()
        );
    }

    match process_botocore_data(botocore_data_path, &simplified_dir) {
        Ok(_processed_count) => {
            // Success
        }
        Err(e) => {
            panic!("Failed to process botocore data: {}", e);
        }
    }

    // Copy the simplified botocore directory to workspace-level target for rust-embed
    let workspace_embed_dir = Path::new("target/botocore-data-simplified");

    // Remove existing directory if it exists
    if workspace_embed_dir.exists() {
        fs::remove_dir_all(workspace_embed_dir)
            .expect("Failed to remove existing botocore embed directory");
    }

    // Copy the simplified directory to the workspace location
    copy_dir_recursive(&simplified_dir, workspace_embed_dir)
        .expect("Failed to copy botocore simplified data");

    // Process boto3 data
    let boto3_data_path = Path::new("resources/config/sdks/boto3/boto3/data");
    if !boto3_data_path.exists() {
        panic!(
            "Required boto3 data directory not found at: {}. Please ensure the boto3 data \
             is available by running `git submodule init && git submodule update`.",
            boto3_data_path.display()
        );
    }

    if let Err(e) = process_boto3_data(boto3_data_path, &boto3_dir) {
        panic!("Failed to process boto3 data: {}", e);
    }

    // Copy the boto3 directory to workspace-level target for rust-embed
    let workspace_boto3_embed_dir = Path::new("target/boto3-data-simplified");

    // Remove existing directory if it exists
    if workspace_boto3_embed_dir.exists() {
        fs::remove_dir_all(workspace_boto3_embed_dir)
            .expect("Failed to remove existing boto3 embed directory");
    }

    // Copy the boto3 directory to the workspace location
    copy_dir_recursive(&boto3_dir, workspace_boto3_embed_dir)
        .expect("Failed to copy boto3 simplified data");
}

fn process_botocore_data(
    botocore_path: &Path,
    output_dir: &Path,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut processed_count = 0;

    // Iterate through service directories
    for entry in fs::read_dir(botocore_path)? {
        let entry = entry?;
        let service_path = entry.path();

        if !service_path.is_dir() {
            continue;
        }

        let service_name = match service_path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        // Find the latest API version for this service
        let latest_version = find_latest_api_version(&service_path)?;

        if let Some((api_version, version_path)) = latest_version {
            // Create output directory for this service/version
            let service_output_dir = output_dir.join(service_name).join(&api_version);
            fs::create_dir_all(&service_output_dir)?;

            // Process files in this version directory
            if process_service_version(&version_path, &service_output_dir)? {
                processed_count += 1;
            }
        }
    }

    Ok(processed_count)
}

fn find_latest_api_version(
    service_path: &Path,
) -> Result<Option<(String, std::path::PathBuf)>, Box<dyn std::error::Error>> {
    let mut versions = Vec::new();

    // Collect all API versions for this service
    for version_entry in fs::read_dir(service_path)? {
        let version_entry = version_entry?;
        let version_path = version_entry.path();

        if !version_path.is_dir() {
            continue;
        }

        let api_version = match version_path.file_name().and_then(|n| n.to_str()) {
            Some(version) => version,
            None => continue,
        };

        versions.push((api_version.to_string(), version_path));
    }

    if versions.is_empty() {
        return Ok(None);
    }

    // Sort versions by version string (assuming YYYY-MM-DD format)
    // This works because lexicographic sorting of YYYY-MM-DD gives chronological order
    versions.sort_by(|a, b| b.0.cmp(&a.0)); // Sort in descending order (latest first)

    Ok(versions.into_iter().next())
}

fn process_service_version(
    version_path: &Path,
    output_dir: &Path,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut has_service_file = false;

    // Process each file in the version directory
    for entry in fs::read_dir(version_path)? {
        let entry = entry?;
        let file_path = entry.path();

        if !file_path.is_file() {
            continue;
        }

        let file_name = match file_path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        match file_name {
            "service-2.json" => {
                // Process and simplify the main service definition
                process_service_definition(&file_path, &output_dir.join(file_name))?;
                has_service_file = true;
            }
            "waiters-2.json" | "paginators-1.json" => {
                // Copy these files as-is (they're already compact)
                fs::copy(&file_path, output_dir.join(file_name))?;
            }
            _ => {
                // Skip other files
                continue;
            }
        }
    }

    Ok(has_service_file)
}

fn process_service_definition(
    input_path: &Path,
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // Read the original service definition
    let content = fs::read_to_string(input_path)?;
    let original: Value = serde_json::from_str(&content)?;

    // Extract version (optional)
    let version = original
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Extract metadata (required)
    let metadata = extract_metadata(original.get("metadata"))?;

    // Convert to simplified structure
    let simplified = SimplifiedServiceDefinition {
        version,
        metadata,
        operations: simplify_operations(original.get("operations"))?,
        shapes: simplify_shapes(original.get("shapes"))?,
    };

    // Write the simplified version as uncompressed JSON
    let simplified_json = serde_json::to_string(&simplified)?;

    // Write uncompressed JSON file (rust-embed will handle compression)
    fs::write(output_path, simplified_json)?;

    Ok(())
}

fn extract_metadata(
    metadata_value: Option<&Value>,
) -> Result<ServiceMetadata, Box<dyn std::error::Error>> {
    if let Some(Value::Object(metadata)) = metadata_value {
        let api_version = metadata
            .get("apiVersion")
            .and_then(|v| v.as_str())
            .ok_or("Missing apiVersion in metadata")?
            .to_string();

        let service_id = metadata
            .get("serviceId")
            .and_then(|v| v.as_str())
            .ok_or("Missing serviceId in metadata")?
            .to_string();

        Ok(ServiceMetadata {
            api_version,
            service_id,
        })
    } else {
        Err("Missing or invalid metadata".into())
    }
}

fn simplify_operations(
    operations_value: Option<&Value>,
) -> Result<HashMap<String, SimplifiedOperation>, Box<dyn std::error::Error>> {
    let mut simplified_operations = HashMap::new();

    if let Some(Value::Object(operations)) = operations_value {
        for (op_name, op_value) in operations {
            let mut simplified_op: SimplifiedOperation = serde_json::from_value(op_value.clone())?;
            simplified_op.name = op_name.clone();
            simplified_operations.insert(op_name.clone(), simplified_op);
        }
    }

    Ok(simplified_operations)
}

fn simplify_shapes(
    shapes_value: Option<&Value>,
) -> Result<HashMap<String, SimplifiedShape>, Box<dyn std::error::Error>> {
    let mut simplified_shapes = HashMap::new();

    if let Some(Value::Object(shapes)) = shapes_value {
        for (shape_name, shape_value) in shapes {
            let simplified_shape: SimplifiedShape = serde_json::from_value(shape_value.clone())?;
            simplified_shapes.insert(shape_name.clone(), simplified_shape);
        }
    }

    Ok(simplified_shapes)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

fn process_boto3_data(
    boto3_path: &Path,
    output_dir: &Path,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut processed_count = 0;

    // Iterate through service directories
    for entry in fs::read_dir(boto3_path)? {
        let entry = entry?;
        let service_path = entry.path();

        if !service_path.is_dir() {
            continue;
        }

        let service_name = match service_path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        // Find the latest API version for this service (reuse existing function)
        let latest_version = find_latest_api_version(&service_path)?;

        if let Some((api_version, version_path)) = latest_version {
            // Create output directory for this service/version
            let service_output_dir = output_dir.join(service_name).join(&api_version);
            fs::create_dir_all(&service_output_dir)?;

            // Process boto3 resources file
            if process_boto3_service_version(&version_path, &service_output_dir)? {
                processed_count += 1;
            }
        }
    }

    Ok(processed_count)
}

fn process_boto3_service_version(
    version_path: &Path,
    output_dir: &Path,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut has_resources_file = false;

    // Process each file in the version directory
    for entry in fs::read_dir(version_path)? {
        let entry = entry?;
        let file_path = entry.path();

        if !file_path.is_file() {
            continue;
        }

        let file_name = match file_path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        match file_name {
            "resources-1.json" => {
                // Copy boto3 resources file as-is (no simplification needed)
                fs::copy(&file_path, output_dir.join(file_name))?;
                has_resources_file = true;
            }
            _ => {
                // Skip other files (boto3 typically only has resources-1.json)
                continue;
            }
        }
    }

    Ok(has_resources_file)
}
