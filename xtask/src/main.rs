use regex::Regex;
use std::{fs, path::Path};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("regenerate-service-names") => regenerate_service_names(),
        _ => {
            eprintln!("Usage: cargo xtask <task>");
            eprintln!("Tasks:");
            eprintln!(
                "  regenerate-service-names  Regenerate tests/java/service_name_test_cases.txt"
            );
            std::process::exit(1);
        }
    }
}

/// Locates `validateServiceIdentifiersForEnvVarsAndProfileProperty()` by method name,
/// walks its body by brace depth, then extracts `(serviceId, javaServiceName)` pairs
/// from each `validateServiceIdSetting(...)` call (first and fourth string arguments).
///
/// Used by both `build.rs` (drift detection) and `xtask` (snapshot regeneration).
fn extract_service_id_settings(content: &str) -> Vec<(String, String)> {
    let re = Regex::new(
        r#"validateServiceIdSetting\(\s*"([^"]+)",\s*"[^"]+",\s*"[^"]+",\s*"([^"]+)"\s*\)"#,
    )
    .unwrap();

    // Find the target method
    let marker = "void validateServiceIdentifiersForEnvVarsAndProfileProperty()";
    let method_pos = content
        .find(marker)
        .expect("validateServiceIdentifiersForEnvVarsAndProfileProperty not found");

    // Find the opening '{' of the method body
    let body_start = content[method_pos..].find('{').unwrap() + method_pos + 1;

    // Walk forward tracking brace depth to find the closing '}'
    let mut depth = 1usize;
    let mut body_end = body_start;
    for ch in content[body_start..].chars() {
        body_end += ch.len_utf8();
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }

    re.captures_iter(&content[body_start..body_end])
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect()
}

fn regenerate_service_names() {
    // CARGO_MANIFEST_DIR is set by Cargo to the xtask crate directory at compile time,
    // so its parent is always the workspace root regardless of where the command is run from.
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest dir has no parent");

    let src = workspace_root.join(
        "iam-policy-autopilot-policy-generation/resources/config/sdks/aws-sdk-java-v2/\
         codegen/src/test/java/software/amazon/awssdk/codegen/naming/DefaultNamingStrategyTest.java",
    );
    let content =
        fs::read_to_string(&src).unwrap_or_else(|e| panic!("Cannot read {}: {e}", src.display()));

    let pairs = extract_service_id_settings(&content);

    let header = "\
# Generated file — do not edit by hand.\n\
# Regenerate with: cargo xtask regenerate-service-names\n\
# Source: aws-sdk-java-v2 DefaultNamingStrategyTest.java — validateServiceIdentifiersForEnvVarsAndProfileProperty()\n\
# Format: serviceId=javaServiceName (fourth arg of validateServiceIdSetting)\n";

    let output: String = std::iter::once(header.to_string())
        .chain(pairs.iter().map(|(id, name)| format!("{id}={name}\n")))
        .collect();

    let dst = workspace_root
        .join("iam-policy-autopilot-policy-generation/tests/java/service_name_test_cases.txt");
    fs::write(&dst, &output).unwrap_or_else(|e| panic!("Cannot write {}: {e}", dst.display()));

    println!("Written {} entries to {}", pairs.len(), dst.display());
}
