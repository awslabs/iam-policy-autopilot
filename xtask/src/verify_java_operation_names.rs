//! Verify the Java SDK v2 operation naming logic against its codegen pipeline.
//!
//! The task checks out the pinned SDK commit into a temporary directory, builds
//! its `codegen` module, and runs a small Java entry point against those classes.
//! The entry point uses the SDK's own C2J loaders and `IntermediateModelBuilder`,
//! which is the part of the codegen pipeline that applies service customizations
//! and determines operation method names.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{ensure, Context, Result};
use iam_policy_autopilot_policy_generation::{Language, ServiceDiscovery};
use tempfile::TempDir;

pub struct VerifyOptions {
    pub java_sdk_root: PathBuf,
}

pub fn run(opts: VerifyOptions) -> Result<()> {
    let temp_dir = TempDir::with_prefix("iam-autopilot-java-codegen-")?;
    let sdk_checkout = temp_dir.path().join("aws-sdk-java-v2");
    export_sdk_checkout(&opts.java_sdk_root, &sdk_checkout, temp_dir.path())?;

    let operation_names_path = temp_dir.path().join("java_operation_names.json");
    run_codegen(&sdk_checkout, &operation_names_path, temp_dir.path())?;
    let operation_names = read_generated_operation_names(&operation_names_path)?;
    verify_operation_names(&operation_names)?;

    println!(
        "Verified {} Java operation names against the SDK codegen pipeline",
        operation_names.len()
    );
    Ok(())
}

fn export_sdk_checkout(
    java_sdk_root: &Path,
    destination: &Path,
    temp_directory: &Path,
) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("Failed to create {}", destination.display()))?;
    let archive = temp_directory.join("aws-sdk-java-v2.tar");

    run_command(
        Command::new("git")
            .arg("-C")
            .arg(java_sdk_root)
            .args(["archive", "--format=tar", "--output"])
            .arg(&archive)
            .arg("HEAD"),
        "export the pinned aws-sdk-java-v2 checkout",
        &temp_directory.join("git-archive.log"),
    )?;
    run_command(
        Command::new("tar")
            .args(["-xf"])
            .arg(&archive)
            .args(["-C"])
            .arg(destination),
        "extract the pinned aws-sdk-java-v2 checkout",
        &temp_directory.join("tar.log"),
    )?;
    fs::remove_file(archive)?;
    Ok(())
}

fn run_codegen(sdk_checkout: &Path, output: &Path, temp_directory: &Path) -> Result<()> {
    let maven_wrapper = sdk_checkout.join("mvnw");
    ensure!(
        maven_wrapper.is_file(),
        "Maven wrapper not found at {}",
        maven_wrapper.display()
    );

    println!("Building the pinned aws-sdk-java-v2 codegen classes...");
    run_command(
        Command::new(&maven_wrapper)
            .current_dir(sdk_checkout)
            .args([
                "--batch-mode",
                "--quiet",
                "-pl",
                "bom-internal,codegen",
                "-am",
                "install",
                "-DskipTests",
                "-Dcheckstyle.skip",
                "-Dspotbugs.skip",
            ]),
        "build the aws-sdk-java-v2 codegen classes",
        &temp_directory.join("maven-build.log"),
    )?;

    let dependency_classpath_file = temp_directory.join("codegen-classpath.txt");
    run_command(
        Command::new(&maven_wrapper)
            .current_dir(sdk_checkout)
            .arg("--batch-mode")
            .arg("--quiet")
            .arg("-f")
            .arg("codegen/pom.xml")
            .arg("org.apache.maven.plugins:maven-dependency-plugin:3.8.1:build-classpath")
            .arg(format!(
                "-Dmdep.outputFile={}",
                dependency_classpath_file.display()
            ))
            .arg("-Dmdep.includeScope=runtime"),
        "resolve the aws-sdk-java-v2 codegen classpath",
        &temp_directory.join("maven-classpath.log"),
    )?;

    let codegen_classes = sdk_checkout.join("codegen").join("target").join("classes");
    ensure!(
        codegen_classes.is_dir(),
        "Codegen classes not found at {}",
        codegen_classes.display()
    );
    let dependency_classpath =
        fs::read_to_string(&dependency_classpath_file).with_context(|| {
            format!(
                "Failed to read Maven classpath from {}",
                dependency_classpath_file.display()
            )
        })?;
    let mut classpath: Vec<PathBuf> = env::split_paths(dependency_classpath.trim()).collect();
    classpath.push(codegen_classes);

    let helper_source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("java")
        .join("JavaOperationNamesGenerator.java");
    ensure!(
        helper_source.is_file(),
        "Java operation-name generator not found at {}",
        helper_source.display()
    );
    let helper_classes = temp_directory.join("helper-classes");
    fs::create_dir_all(&helper_classes)?;

    let compile_classpath = join_classpath(&classpath)?;
    run_command(
        Command::new("javac")
            .arg("-cp")
            .arg(&compile_classpath)
            .arg("-d")
            .arg(&helper_classes)
            .arg(&helper_source),
        "compile the Java operation-name generator",
        &temp_directory.join("javac.log"),
    )?;

    classpath.insert(0, helper_classes);
    let runtime_classpath = join_classpath(&classpath)?;
    println!("Computing operation names with the Java SDK codegen pipeline...");
    run_command(
        Command::new("java")
            .arg("-cp")
            .arg(runtime_classpath)
            .arg("JavaOperationNamesGenerator")
            .arg(sdk_checkout.join("services"))
            .arg(output),
        "run the Java operation-name generator",
        &temp_directory.join("java-generator.log"),
    )?;

    Ok(())
}

fn join_classpath(paths: &[PathBuf]) -> Result<OsString> {
    env::join_paths(paths).context("Failed to construct the Java classpath")
}

fn run_command(command: &mut Command, description: &str, log_path: &Path) -> Result<()> {
    let stdout = File::create(log_path)
        .with_context(|| format!("Failed to create {}", log_path.display()))?;
    let stderr = stdout
        .try_clone()
        .with_context(|| format!("Failed to clone {}", log_path.display()))?;
    let status = command
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .status()
        .with_context(|| format!("Failed to {description}"))?;

    if !status.success() {
        let log = fs::read_to_string(log_path).unwrap_or_default();
        let tail = log.lines().rev().take(80).collect::<Vec<_>>();
        anyhow::bail!(
            "Failed to {description} (status {status}). Last log lines:\n{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        );
    }
    Ok(())
}

fn read_generated_operation_names(path: &Path) -> Result<BTreeMap<String, String>> {
    let mappings: BTreeMap<String, String> = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?,
    )
    .with_context(|| format!("Failed to parse {}", path.display()))?;
    ensure!(
        !mappings.is_empty(),
        "Java codegen produced no operation mappings in {}",
        path.display()
    );
    Ok(mappings)
}

fn verify_operation_names(operation_names: &BTreeMap<String, String>) -> Result<()> {
    let mut mismatches = Vec::new();

    for (operation_name, sdk_method_name) in operation_names {
        let rust_method_name =
            ServiceDiscovery::operation_to_method_name(operation_name, Language::Java);
        if rust_method_name != *sdk_method_name {
            mismatches.push(format!(
                "{operation_name}: Java SDK generated {sdk_method_name}, Rust generated {rust_method_name}"
            ));
        }

        let rust_operation_name = ServiceDiscovery::java_method_to_operation_name(sdk_method_name);
        if rust_operation_name != *operation_name {
            mismatches.push(format!(
                "{sdk_method_name}: cannot reverse Java SDK operation {operation_name}; Rust generated {rust_operation_name}"
            ));
        }
    }

    ensure!(
        mismatches.is_empty(),
        "Java operation naming has drifted:\n{}",
        mismatches.join("\n")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_generated_operation_names() {
        let temp_dir = TempDir::new().expect("temporary directory should be created");
        let map = temp_dir.path().join("java-operation-names.json");
        fs::write(&map, r#"{"CreateDBCluster":"createDBCluster"}"#)
            .expect("operation names should be written");

        let mappings = read_generated_operation_names(&map).expect("operation names should load");
        assert_eq!(
            mappings.get("CreateDBCluster").map(String::as_str),
            Some("createDBCluster")
        );
        verify_operation_names(&mappings).expect("operation names should match");
    }

    #[test]
    fn rejects_drift() {
        let mappings =
            BTreeMap::from([("CreateDBCluster".to_string(), "createDbCluster".to_string())]);
        let error = verify_operation_names(&mappings).expect_err("drift should fail");
        assert!(error
            .to_string()
            .contains("Java operation naming has drifted"));
    }
}
