//! Consolidated Terraform resource resolver — single-struct association of parsed
//! resources with ARN bindings, source locations, resolved variables, and policy
//! substitution logic.
//!
//! This module provides a single entry point (`TerraformResourceResolver::from_directory`)
//! that handles:
//! - HCL parsing
//! - Variable resolution
//! - Source tracing
//! - State file parsing
//! - IAM service mapping
//! - ARN derivation (HCL + state)
//! - ARN substitution into enriched SDK calls
//! - Binding explanation generation

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use convert_case::{Case, Casing};
use log::debug;
use regex::Regex;

use crate::enrichment::ServiceReferenceLoader;
use crate::Location;
use crate::enrichment::{Action, EnrichedSdkMethodCall, Resource};

use crate::extraction::terraform::state_parser::TerraformStateResources;
use crate::extraction::terraform::variable_resolver::VariableContext;
use crate::extraction::terraform::{AttributeValue, TerraformResources, TerraformResource};

use super::{BindingSource, ResourceBindingExplanation};

// ---------------------------------------------------------------------------
// Shared utilities for ARN placeholder handling
// ---------------------------------------------------------------------------

/// Infrastructure-level ARN placeholders that are NOT resource-specific.
/// These are resolved by `ArnParser` later, not by resource binding.
const AWS_PLACEHOLDERS: &[&str] = &["partition", "region", "account"];

/// Regex to match ARN placeholder variables like `${BucketName}`.
static PLACEHOLDER_RE: OnceLock<Regex> = OnceLock::new();

/// Returns a shared regex for matching `${...}` placeholders in ARN patterns.
fn placeholder_regex() -> &'static Regex {
    PLACEHOLDER_RE.get_or_init(|| Regex::new(r"\$\{([^}]+)\}").expect("valid regex"))
}

// ---------------------------------------------------------------------------
// Resolved resource model
// ---------------------------------------------------------------------------

/// A fully-resolved Terraform resource with all associated metadata.
#[derive(Debug, Clone)]
pub struct ResolvedTerraformResource {
    /// The original parsed resource (includes location).
    pub resource: TerraformResource,
    /// IAM service name derived from `names_data.hcl` (e.g. `"s3"`).
    pub service_name: Option<String>,
    /// IAM resource suffix derived from `names_data.hcl` (e.g. `"bucket"`).
    pub resource_type: Option<String>,
    /// Concrete ARN from `terraform.tfstate`. Takes precedence over `hcl_arn`.
    pub state_arn: Option<String>,
    /// Location of the state ARN in the `.tfstate` file.
    pub state_arn_location: Option<Location>,
    /// ARN constructed from HCL attributes + SDF patterns. Fallback when `state_arn` is absent.
    pub hcl_arn: Option<String>,
    /// The resolved naming attribute value (e.g. `"my-app-data-bucket"`).
    pub binding_name: Option<String>,
    /// Handler entry point if applicable (e.g. `"index.handler"`).
    pub handler: Option<String>,
}

/// Map key: `(service_name, resource_type)` e.g. `("s3", "bucket")`.
pub type ResolvedResourceKey = (String, String);

/// Indexed map of resolved resources keyed by `(service_name, resource_type)`.
/// Multiple terraform resources can map to the same IAM key (e.g. two S3 buckets).
pub type ResolvedResourceMap = HashMap<ResolvedResourceKey, Vec<ResolvedTerraformResource>>;

impl ResolvedTerraformResource {
    /// Build from a parsed resource with no enrichment.
    #[must_use]
    pub fn from_parsed(resource: TerraformResource) -> Self {
        Self {
            resource,
            service_name: None,
            resource_type: None,
            state_arn: None,
            state_arn_location: None,
            hcl_arn: None,
            binding_name: None,
            handler: None,
        }
    }

    /// The best available ARN: state ARN if present, otherwise HCL-derived.
    #[must_use]
    pub fn effective_arn(&self) -> Option<&str> {
        self.state_arn.as_deref().or(self.hcl_arn.as_deref())
    }

    /// Whether this resource has any ARN (state or HCL).
    #[must_use]
    pub fn has_arn(&self) -> bool {
        self.effective_arn().is_some()
    }

    /// Whether this resource was mapped to an IAM service.
    #[must_use]
    pub fn has_service_reference_mapping(&self) -> bool {
        self.service_name.is_some()
    }
}

// ---------------------------------------------------------------------------
// TerraformResourceResolver — the main entry point
// ---------------------------------------------------------------------------

/// Consolidated Terraform resource resolver.
///
/// Owns the full lifecycle: parsing Terraform, resolving variables, tracing sources,
/// loading state, mapping to IAM services, deriving ARNs, and substituting
/// concrete resource names into enriched SDK calls.
#[derive(Debug)]
pub struct TerraformResourceResolver {
    /// All resolved resources keyed by `(service_name, resource_type)`.
    resources: ResolvedResourceMap,
    /// Parse warnings from HCL parsing.
    warnings: Vec<String>,
}

impl TerraformResourceResolver {
    /// Build a resolver from a combination of Terraform inputs.
    ///
    /// This is the primary factory method that supports:
    /// - An optional directory containing `.tf` files (discovered recursively)
    /// - Individual `.tf` files (combined with directory-discovered files)
    /// - Multiple `terraform.tfstate` files for deployed ARN resolution
    ///
    /// Steps:
    /// 1. Parses `.tf` files from the directory (if provided) and individual files
    /// 2. Resolves `var.xxx` references from defaults + `.tfvars` files
    /// 3. Traces source code from compute resources (e.g. Lambda handlers)
    /// 4. Parses `terraform.tfstate` files for deployed ARNs
    /// 5. Resolves each HCL resource to its service, ARN, and metadata
    pub async fn new(
        terraform_dir: Option<&Path>,
        terraform_files: &[PathBuf],
        tfstate_paths: &[PathBuf],
        loader: &ServiceReferenceLoader,
    ) -> Result<Self> {
        // Step 1: Parse Terraform HCL files from directory and individual files
        let mut tf_result = TerraformResources::default();

        if let Some(directory) = terraform_dir {
            tf_result
                .from_directory(directory)
                .context("Failed to parse Terraform directory")?;
        }

        tf_result
            .from_files(terraform_files)
            .context("Failed to parse individual Terraform files")?;

        for warning in tf_result.warnings() {
            log::warn!("Terraform parse warning: {warning}");
        }

        // Step 2: Resolve variables (only when a directory is provided)
        if let Some(directory) = terraform_dir {
            let var_ctx = VariableContext::from_directory(directory).unwrap_or_else(|e| {
                log::warn!("Failed to resolve Terraform variables: {e}");
                VariableContext::default()
            });
            var_ctx.resolve_attributes(&mut tf_result);
        }

        debug!("Parsed {} Terraform resources", tf_result.len());

        // Step 3: Parse terraform.tfstate files
        let state_resources = TerraformStateResources::from_files(tfstate_paths)
            .context("Failed to parse Terraform state files")?;
        debug!("Parsed {} state resource groups", state_resources.len());

        // Step 4: Resolve all resources
        let resources =
            resolve_terraform_resources(&tf_result, &state_resources, &VariableContext::default(), loader).await;

        debug!(
            "Resolved {} resource groups from Terraform",
            resources.len()
        );

        Ok(Self {
            resources,
            warnings: tf_result.take_warnings(),
        })
    }

    /// Build a resolver from a Terraform project directory.
    ///
    /// Convenience method that wraps [`Self::new`] for the common case of a single
    /// directory with an optional single tfstate file.
    pub async fn from_directory(
        directory: &Path,
        tfstate_path: Option<&PathBuf>,
        loader: &ServiceReferenceLoader,
    ) -> Result<Self> {
        let tfstate_paths: Vec<PathBuf> = tfstate_path.into_iter().cloned().collect();
        Self::new(Some(directory), &[], &tfstate_paths, loader).await
    }

    /// Build a resolver directly from pre-computed components (useful for testing).
    #[cfg(test)]
    pub fn from_resolved_map(resources: ResolvedResourceMap) -> Self {
        Self {
            resources,
            warnings: Vec::new(),
        }
    }

    /// Access the resolved resource map.
    #[must_use]
    pub fn resources(&self) -> &ResolvedResourceMap {
        &self.resources
    }

    /// Access parse warnings.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Returns `true` if no IAM-mappable resources were resolved.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    /// Number of distinct `(service, resource_type)` groups resolved.
    #[must_use]
    pub fn len(&self) -> usize {
        self.resources.len()
    }

    // -----------------------------------------------------------------------
    // ARN substitution
    // -----------------------------------------------------------------------

    /// Public wrapper around the internal `substitute_arn_patterns` for integration tests.
    #[cfg(any(test, feature = "integ-test"))]
    pub fn substitute_arn_patterns_for_test(
        &self,
        service: &str,
        resource_type_name: &str,
        arn_patterns: &[String],
    ) -> Option<Vec<String>> {
        self.substitute_arn_patterns(service, resource_type_name, arn_patterns)
    }

    /// Apply Terraform resource bindings to enriched SDK calls.
    ///
    /// For each action's resources, if the resource type has a matching binding,
    /// substitute the ARN patterns with concrete values. Otherwise, leave unchanged.
    pub fn substitute_enriched_calls<'a>(
        &self,
        enriched_calls: &[EnrichedSdkMethodCall<'a>],
    ) -> Vec<EnrichedSdkMethodCall<'a>> {
        if self.resources.is_empty() {
            return enriched_calls.to_vec();
        }

        enriched_calls
            .iter()
            .map(|call| {
                let new_actions: Vec<Action> = call
                    .actions
                    .iter()
                    .map(|action| {
                        let service = action.service();
                        let new_resources: Vec<Resource> = action
                            .resources
                            .iter()
                            .map(|resource| {
                                if let Some(arn_patterns) = &resource.arn_patterns {
                                    if let Some(substituted) = self.substitute_arn_patterns(
                                        service,
                                        &resource.name,
                                        arn_patterns,
                                    ) {
                                        return Resource::new(
                                            resource.name.clone(),
                                            Some(substituted),
                                        );
                                    }
                                }
                                resource.clone()
                            })
                            .collect();

                        Action::new(
                            action.name.clone(),
                            new_resources,
                            action.conditions.clone(),
                            action.explanation.clone(),
                        )
                    })
                    .collect();

                EnrichedSdkMethodCall {
                    method_name: call.method_name.clone(),
                    service: call.service.clone(),
                    actions: new_actions,
                    sdk_method_call: call.sdk_method_call,
                }
            })
            .collect()
    }

    /// Substitute resource-specific placeholders in ARN patterns with concrete values.
    ///
    /// Resolution priority:
    /// 1. State-derived full ARNs (exact, from `terraform.tfstate`)
    /// 2. HCL-derived binding names (substituted into pattern templates)
    ///
    /// For sub-resources (ARN patterns with multiple resource-specific placeholders,
    /// e.g. S3 objects), falls back to the parent resource type inferred from the
    /// first placeholder name.
    ///
    /// Returns `None` when no useful substitution can be made.
    ///
    /// Exposed as public for integration tests via `substitute_arn_patterns_for_test`.
    fn substitute_arn_patterns(
        &self,
        service: &str,
        resource_type_name: &str,
        arn_patterns: &[String],
    ) -> Option<Vec<String>> {
        let is_sub_resource = arn_patterns_have_multiple_resource_placeholders(arn_patterns);

        // --- Priority 1: State-derived full ARNs ---
        if let Some(full_arns) = self
            .lookup_full_arns(service, resource_type_name)
            .or_else(|| {
                if is_sub_resource {
                    self.lookup_full_arns_by_first_placeholder(service, arn_patterns)
                } else {
                    None
                }
            })
        {
            if !full_arns.is_empty() {
                // For sub-resources matched via parent, append /*
                if is_sub_resource && self.lookup_full_arns(service, resource_type_name).is_none() {
                    let sub_arns: Vec<String> =
                        full_arns.iter().map(|a| format!("{a}/*")).collect();
                    return Some(sub_arns);
                }
                return Some(full_arns.clone());
            }
        }

        // --- Priority 2: HCL-derived binding names ---
        let names = self
            .lookup_binding_names(service, resource_type_name)
            .or_else(|| {
                if is_sub_resource {
                    self.lookup_binding_names_by_first_placeholder(service, arn_patterns)
                } else {
                    None
                }
            })?;

        // Single wildcard binding means no improvement over default
        if names.len() == 1 && names[0] == "*" {
            return None;
        }

        let regex = placeholder_regex();
        let mut result = Vec::new();

        for pattern in arn_patterns {
            let resource_placeholder_count = regex
                .captures_iter(pattern)
                .filter(|cap| {
                    let name = cap.get(1).map_or("", |m| m.as_str());
                    !is_aws_placeholder(name)
                })
                .count();

            if resource_placeholder_count == 0 {
                result.push(pattern.clone());
                continue;
            }

            for concrete_name in &names {
                let mut first_replaced = false;
                let substituted = regex
                    .replace_all(pattern, |caps: &regex::Captures| {
                        let placeholder = caps.get(1).map_or("", |m| m.as_str());
                        if is_aws_placeholder(placeholder) {
                            format!("${{{placeholder}}}")
                        } else if !first_replaced {
                            first_replaced = true;
                            concrete_name.clone()
                        } else {
                            "*".to_string()
                        }
                    })
                    .to_string();
                result.push(substituted);
            }
        }

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    /// Look up concrete binding names for a given IAM service and resource type.
    ///
    /// Collects `binding_name` values from resolved resources matching the key,
    /// falling back to `"*"` for resources with unresolvable expressions.
    fn lookup_binding_names(&self, service: &str, resource_type: &str) -> Option<Vec<String>> {
        let key = (service.to_string(), resource_type.to_string());
        let resources = self.resources.get(&key)?;

        let mut names: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for r in resources {
            let name = r.binding_name.clone().unwrap_or_else(|| "*".to_string());
            if seen.insert(name.clone()) {
                names.push(name);
            }
        }

        if names.is_empty() {
            None
        } else {
            Some(names)
        }
    }

    /// Look up full ARNs from state for a given service and resource type.
    fn lookup_full_arns(&self, service: &str, resource_type: &str) -> Option<Vec<String>> {
        let key = (service.to_string(), resource_type.to_string());
        let resources = self.resources.get(&key)?;

        let arns: Vec<String> = resources
            .iter()
            .filter_map(|r| r.state_arn.clone())
            .collect();

        if arns.is_empty() {
            None
        } else {
            Some(arns)
        }
    }

    /// Fallback lookup by matching the first resource-specific placeholder in the
    /// ARN patterns against stored binding keys for the given service.
    ///
    /// Used for sub-resources (e.g., `s3/object` → look up `s3/bucket`
    /// by matching the `BucketName` placeholder against known resource type names).
    fn lookup_binding_names_by_first_placeholder(
        &self,
        service: &str,
        arn_patterns: &[String],
    ) -> Option<Vec<String>> {
        let first_placeholder = first_resource_placeholder(arn_patterns)?;
        for ((s, r), resources) in &self.resources {
            if s == service
                && first_placeholder
                    .to_ascii_lowercase()
                    .contains(&r.to_ascii_lowercase())
            {
                let mut names: Vec<String> = Vec::new();
                let mut seen: HashSet<String> = HashSet::new();
                for res in resources {
                    let name = res.binding_name.clone().unwrap_or_else(|| "*".to_string());
                    if seen.insert(name.clone()) {
                        names.push(name);
                    }
                }
                if !names.is_empty() {
                    return Some(names);
                }
            }
        }
        None
    }

    /// Same as `lookup_binding_names_by_first_placeholder` but for full ARNs from state.
    fn lookup_full_arns_by_first_placeholder(
        &self,
        service: &str,
        arn_patterns: &[String],
    ) -> Option<Vec<String>> {
        let first_placeholder = first_resource_placeholder(arn_patterns)?;
        for ((s, r), resources) in &self.resources {
            if s == service
                && first_placeholder
                    .to_ascii_lowercase()
                    .contains(&r.to_ascii_lowercase())
            {
                let arns: Vec<String> = resources
                    .iter()
                    .filter_map(|res| res.state_arn.clone())
                    .collect();
                if !arns.is_empty() {
                    return Some(arns);
                }
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // Binding explanations
    // -----------------------------------------------------------------------

    /// Build explanations for where resource ARNs came from.
    ///
    /// Uses the pre-computed `hcl_arn`, `state_arn`, and resource metadata
    /// on each `ResolvedTerraformResource`, avoiding re-derivation.
    #[must_use]
    pub fn build_binding_explanations(&self) -> Vec<ResourceBindingExplanation> {
        let mut explanations = Vec::new();

        for resources in self.resources.values() {
            for resolved in resources {
                let resource_location = &resolved.resource.location;

                // State-derived ARN explanation
                if let Some(ref state_arn) = resolved.state_arn {
                    let location = resolved
                        .state_arn_location
                        .clone()
                        .unwrap_or_else(|| resource_location.clone());

                    explanations.push(ResourceBindingExplanation {
                        arn: state_arn.clone(),
                        source: BindingSource::TerraformState,
                        resource_type: resolved.resource.resource_type.clone(),
                        resource_name: resolved.resource.local_name.clone(),
                        location,
                    });
                    continue;
                }

                // Terraform-derived ARN explanation
                if let Some(ref hcl_arn) = resolved.hcl_arn {
                    // Skip if state ARN will also be emitted (state takes precedence)
                    if resolved.state_arn.is_none() {
                        explanations.push(ResourceBindingExplanation {
                            arn: hcl_arn.clone(),
                            source: BindingSource::Terraform,
                            resource_type: resolved.resource.resource_type.clone(),
                            resource_name: resolved.resource.local_name.clone(),
                            location: resource_location.clone(),
                        });
                    }
                }

                
            }
        }

        explanations
    }
}

// ---------------------------------------------------------------------------
// Resource resolution (internal)
// ---------------------------------------------------------------------------

/// Build a `ResolvedResourceMap` from parsed HCL resources, enriched with
/// state data, variable resolution, and SDF ARN patterns.
///
/// Keyed by `(service_name, resource_type)` for direct lookup during
/// policy generation. 
/// 
/// Resources that don't map to a service in names_data.hcl are excluded.
async fn resolve_terraform_resources(
    terraform_resources: &TerraformResources,
    state_resources: &TerraformStateResources,
    var_ctx: &VariableContext,
    loader: &ServiceReferenceLoader,
) -> ResolvedResourceMap {
    let resolver = super::service_resolver::TerraformServiceAndResourceResolver::global();
    let mut results = ResolvedResourceMap::new();

    for resource in terraform_resources.values() {
        let mut resolved_res = ResolvedTerraformResource::from_parsed(resource.clone());
        let tf_key = (resource.resource_type.clone(), resource.local_name.clone());

        let Some((service, resource_type)) = resolver.resolve(&resource.resource_type) else {
            continue;
        };
        resolved_res.service_name = Some(service.clone());
        resolved_res.resource_type = Some(resource_type.clone());

        // Derive HCL ARN from SDF patterns + naming attribute
        if let Some(patterns) = loader.get_resource_arns(&service, &resource_type).await {
            if let Some(naming_attr) = derive_naming_attribute(&patterns, &resource.attributes) {
                if let Some(attr_value) = resource.attributes.get(&naming_attr) {
                    let resolved_value = match attr_value {
                        AttributeValue::Literal(s) => Some(s.clone()),
                        AttributeValue::Expression(_) => {
                            var_ctx.try_resolve(attr_value).and_then(|v| match v {
                                AttributeValue::Literal(s) => Some(s),
                                _ => None,
                            })
                        }
                    };
                    if let Some(name) = resolved_value {
                        resolved_res.binding_name = Some(name.clone());
                        if let Some(pattern) = patterns.first() {
                            let re = placeholder_regex();
                            let mut replaced = false;
                            let hcl_arn = re
                                .replace_all(pattern, |caps: &regex::Captures| {
                                    let ph = caps.get(1).map_or("", |m| m.as_str());
                                    if is_aws_placeholder(ph) {
                                        format!("${{{ph}}}")
                                    } else if !replaced {
                                        replaced = true;
                                        name.clone()
                                    } else {
                                        "*".to_string()
                                    }
                                })
                                .to_string();
                            resolved_res.hcl_arn = Some(hcl_arn);
                        }
                    } else {
                        // Expression couldn't be resolved — use wildcard as binding name
                        resolved_res.binding_name = Some("*".to_string());
                    }
                }
            }
        }

        // Attach state ARN if available
        if let Some(resources) = state_resources.get(&tf_key.0, &tf_key.1) {
            if let Some(resource) = resources.iter().find(|s| s.arn.is_some()) {
                resolved_res.state_arn = resource.arn.clone();
                resolved_res.state_arn_location = resource.arn_location.clone();
            }
        }

        results
            .entry((service, resource_type))
            .or_default()
            .push(resolved_res);
    }

    results
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Returns `true` if the placeholder name is an infrastructure-level placeholder.
fn is_aws_placeholder(name: &str) -> bool {
    AWS_PLACEHOLDERS
        .iter()
        .any(|p| p.eq_ignore_ascii_case(name))
}

/// Returns `true` if the ARN patterns contain more than one resource-specific
/// placeholder (i.e., the resource is a sub-resource of another, like S3 objects
/// inside a bucket).
fn arn_patterns_have_multiple_resource_placeholders(arn_patterns: &[String]) -> bool {
    let regex = placeholder_regex();
    arn_patterns.iter().any(|pattern| {
        regex
            .captures_iter(pattern)
            .filter(|cap| {
                let name = cap.get(1).map_or("", |m| m.as_str());
                !is_aws_placeholder(name)
            })
            .count()
            > 1
    })
}

/// Extract the name of the first resource-specific placeholder from ARN patterns.
fn first_resource_placeholder(arn_patterns: &[String]) -> Option<String> {
    let regex = placeholder_regex();
    for pattern in arn_patterns {
        for cap in regex.captures_iter(pattern) {
            let name = cap.get(1).map_or("", |m| m.as_str());
            if !is_aws_placeholder(name) {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Derive the naming attribute from ARN format placeholders.
///
/// Given ARN patterns like `["arn:${Partition}:s3:::${BucketName}"]`, extracts
/// the first resource-specific placeholder (`BucketName`), converts it to
/// snake_case candidates (`bucket_name`, `bucket`), and returns the first one
/// that exists in the resource's attributes.
///
/// Returns `None` if no matching attribute is found — callers should treat this
/// as "unknown" and skip the resource rather than guessing.
pub(crate) fn derive_naming_attribute(
    arn_patterns: &[String],
    attributes: &HashMap<String, AttributeValue>,
) -> Option<String> {
    let regex = placeholder_regex();

    for pattern in arn_patterns {
        for cap in regex.captures_iter(pattern) {
            let placeholder = cap.get(1).map_or("", |m| m.as_str());
            if is_aws_placeholder(placeholder) {
                continue;
            }

            let candidates = placeholder_to_attribute_candidates(placeholder);
            for candidate in &candidates {
                if attributes.contains_key(candidate) {
                    return Some(candidate.clone());
                }
            }
        }
    }

    None
}

/// Convert an ARN placeholder like `BucketName` to Terraform attribute candidates.
///
/// Examples:
/// - `BucketName` → `["bucket_name", "bucket", "name"]`
/// - `FunctionName` → `["function_name", "function", "name"]`
/// - `TableName` → `["table_name", "table", "name"]`
fn placeholder_to_attribute_candidates(placeholder: &str) -> Vec<String> {
    let snake = placeholder.to_case(Case::Snake);
    let mut candidates = vec![snake.clone()];

    if let Some(without_name) = snake.strip_suffix("_name") {
        candidates.push(without_name.to_string());
    }
    if let Some(without_id) = snake.strip_suffix("_id") {
        candidates.push(without_id.to_string());
    }

    candidates.push("name".to_string());

    candidates
}


// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::BindingSource;
    use super::*;
    use crate::enrichment::{Action, EnrichedSdkMethodCall, Resource};
    use crate::extraction::terraform::state_parser::StateResource;
    use crate::extraction::terraform::{AttributeValue, TerraformResource};
    use crate::Explanation;
    use crate::SdkMethodCall;
    use rstest::rstest;
    use std::collections::HashMap;
    use std::path::PathBuf;

    // -----------------------------------------------------------------------
    // Test fixture helpers
    // -----------------------------------------------------------------------

    fn vec_to_terraform_resources(resources: Vec<TerraformResource>) -> TerraformResources {
        let mut terraform_resources = TerraformResources::default();
        for r in resources {
            terraform_resources.insert(r);
        }
        terraform_resources
    }

    fn make_hcl_resource(
        rtype: &str,
        name: &str,
        attr_key: &str,
        attr_val: AttributeValue,
    ) -> TerraformResource {
        TerraformResource {
            resource_type: rtype.to_string(),
            local_name: name.to_string(),
            attributes: HashMap::from([(attr_key.to_string(), attr_val)]),
            location: Location::new(PathBuf::from("main.tf"), (10, 1), (10, 1)),
        }
    }

    fn make_state_resources(entries: Vec<(&str, &str, Option<&str>)>) -> TerraformStateResources {
        let mut state_resources = TerraformStateResources::default();
        for (rtype, name, arn) in entries {
            state_resources.push(StateResource {
                resource_type: rtype.to_string(),
                name: name.to_string(),
                arn: arn.map(String::from),
                arn_location: None,
            });
        }
        state_resources
    }

    fn empty_var_ctx() -> VariableContext {
        VariableContext::default()
    }

    fn make_sdk_call() -> SdkMethodCall {
        SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: None,
        }
    }

    fn make_resolved_resource(
        rtype: &str,
        local_name: &str,
        service: &str,
        suffix: &str,
        binding_name: Option<&str>,
        state_arn: Option<&str>,
        hcl_arn: Option<&str>,
        state_arn_location: Option<Location>,
    ) -> ResolvedTerraformResource {
        ResolvedTerraformResource {
            resource: TerraformResource {
                resource_type: rtype.to_string(),
                local_name: local_name.to_string(),
                attributes: HashMap::new(),
                location: Location::new(PathBuf::from("main.tf"), (10, 1), (10, 1)),
            },
            service_name: Some(service.to_string()),
            resource_type: Some(suffix.to_string()),
            state_arn: state_arn.map(String::from),
            state_arn_location: state_arn_location,
            hcl_arn: hcl_arn.map(String::from),
            binding_name: binding_name.map(String::from),
            handler: None
        }
    }

    // -----------------------------------------------------------------------
    // ResolvedTerraformResource tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_from_parsed_sets_location() {
        let r = make_hcl_resource(
            "aws_s3_bucket",
            "data",
            "bucket",
            AttributeValue::Literal("x".into()),
        );
        let resolved = ResolvedTerraformResource::from_parsed(r);
        assert_eq!(resolved.resource.location, Location::new(PathBuf::from("main.tf"), (10, 1), (10, 1)));
        assert!(!resolved.has_service_reference_mapping());
        assert!(!resolved.has_arn());
    }

    #[test]
    fn test_effective_arn_prefers_state() {
        let mut r = ResolvedTerraformResource::from_parsed(make_hcl_resource(
            "aws_s3_bucket",
            "b",
            "bucket",
            AttributeValue::Literal("x".into()),
        ));
        r.hcl_arn = Some("arn:${Partition}:s3:::x".into());
        r.state_arn = Some("arn:aws:s3:::real-bucket".into());
        assert_eq!(r.effective_arn(), Some("arn:aws:s3:::real-bucket"));
    }

    #[test]
    fn test_effective_arn_falls_back_to_hcl() {
        let mut r = ResolvedTerraformResource::from_parsed(make_hcl_resource(
            "aws_s3_bucket",
            "b",
            "bucket",
            AttributeValue::Literal("x".into()),
        ));
        r.hcl_arn = Some("arn:${Partition}:s3:::x".into());
        assert_eq!(r.effective_arn(), Some("arn:${Partition}:s3:::x"));
    }

    // -----------------------------------------------------------------------
    // resolve_terraform_resources tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_resolve_attaches_state_arn() {
        let hcl = vec![make_hcl_resource(
            "aws_s3_bucket",
            "data",
            "bucket",
            AttributeValue::Literal("my-bucket".into()),
        )];
        let state = make_state_resources(vec![(
            "aws_s3_bucket",
            "data",
            Some("arn:aws:s3:::my-bucket"),
        )]);
        let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
        let resolved = resolve_terraform_resources(
            &vec_to_terraform_resources(hcl),
            &state,
            &empty_var_ctx(),
            &loader,
        )
        .await;
        let s3 = &resolved[&("s3".into(), "bucket".into())];
        assert_eq!(s3.len(), 1);
        assert_eq!(s3[0].state_arn.as_deref(), Some("arn:aws:s3:::my-bucket"));
    }

    #[tokio::test]
    async fn test_resolve_no_state_match() {
        let hcl = vec![make_hcl_resource(
            "aws_s3_bucket",
            "data",
            "bucket",
            AttributeValue::Literal("x".into()),
        )];
        let state = make_state_resources(vec![("aws_s3_bucket", "other", Some("arn:aws:s3:::other"))]);
        let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
        let resolved = resolve_terraform_resources(
            &vec_to_terraform_resources(hcl),
            &state,
            &empty_var_ctx(),
            &loader,
        )
        .await;
        let s3 = &resolved[&("s3".into(), "bucket".into())];
        assert!(s3[0].state_arn.is_none());
    }

    #[tokio::test]
    async fn test_resolve_iam_mapping() {
        let hcl = vec![make_hcl_resource(
            "aws_s3_bucket",
            "data",
            "bucket",
            AttributeValue::Literal("x".into()),
        )];
        let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
        let resolved = resolve_terraform_resources(
            &vec_to_terraform_resources(hcl),
            &TerraformStateResources::default(),
            &empty_var_ctx(),
            &loader,
        )
        .await;
        assert!(resolved.contains_key(&("s3".into(), "bucket".into())));
        assert_eq!(
            resolved[&("s3".into(), "bucket".into())][0]
                .service_name
                .as_deref(),
            Some("s3")
        );
    }

    #[tokio::test]
    async fn test_resolve_empty_inputs() {
        let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
        let resolved = resolve_terraform_resources(
            &TerraformResources::default(),
            &TerraformStateResources::default(),
            &empty_var_ctx(),
            &loader,
        )
        .await;
        assert!(resolved.is_empty());
    }

    #[tokio::test]
    async fn test_non_aws_resources_excluded() {
        let hcl = vec![make_hcl_resource(
            "null_resource",
            "x",
            "name",
            AttributeValue::Literal("y".into()),
        )];
        let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
        let resolved = resolve_terraform_resources(
            &vec_to_terraform_resources(hcl),
            &TerraformStateResources::default(),
            &empty_var_ctx(),
            &loader,
        )
        .await;
        assert!(resolved.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_resources_same_iam_type() {
        let hcl = vec![
            make_hcl_resource(
                "aws_s3_bucket",
                "a",
                "bucket",
                AttributeValue::Literal("bucket-a".into()),
            ),
            make_hcl_resource(
                "aws_s3_bucket",
                "b",
                "bucket",
                AttributeValue::Literal("bucket-b".into()),
            ),
        ];
        let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
        let resolved = resolve_terraform_resources(
            &vec_to_terraform_resources(hcl),
            &TerraformStateResources::default(),
            &empty_var_ctx(),
            &loader,
        )
        .await;
        let s3 = &resolved[&("s3".into(), "bucket".into())];
        assert_eq!(s3.len(), 2);
    }

    // -----------------------------------------------------------------------
    // substitute_enriched_calls tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_substitute_enriched_calls_replaces_arns() {
        let sdk_call = make_sdk_call();

        let enriched = vec![EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![Action::new(
                "s3:GetObject".to_string(),
                vec![Resource::new(
                    "bucket".to_string(),
                    Some(vec!["arn:${Partition}:s3:::${BucketName}".to_string()]),
                )],
                vec![],
                Explanation::default(),
            )],
            sdk_method_call: &sdk_call,
        }];

        let mut resource_map = ResolvedResourceMap::new();
        resource_map
            .entry(("s3".to_string(), "bucket".to_string()))
            .or_default()
            .push(make_resolved_resource(
                "aws_s3_bucket",
                "data",
                "s3",
                "bucket",
                Some("my-app-data"),
                None,
                Some("arn:${Partition}:s3:::my-app-data"),
                None
            ));
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);
        let result = resolver.substitute_enriched_calls(&enriched);

        assert_eq!(result.len(), 1);
        let arn_patterns = result[0].actions[0].resources[0]
            .arn_patterns
            .as_ref()
            .unwrap();
        assert_eq!(arn_patterns, &["arn:${Partition}:s3:::my-app-data"]);
    }

    #[tokio::test]
    async fn test_substitute_enriched_calls_no_match_keeps_original() {
        let sdk_call = make_sdk_call();

        let enriched = vec![EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![Action::new(
                "s3:GetObject".to_string(),
                vec![Resource::new(
                    "bucket".to_string(),
                    Some(vec!["arn:${Partition}:s3:::${BucketName}".to_string()]),
                )],
                vec![],
                Explanation::default(),
            )],
            sdk_method_call: &sdk_call,
        }];

        // Empty resources → no bindings
        let resolver = TerraformResourceResolver::from_resolved_map(ResolvedResourceMap::new());
        let result = resolver.substitute_enriched_calls(&enriched);

        let arn_patterns = result[0].actions[0].resources[0]
            .arn_patterns
            .as_ref()
            .unwrap();
        assert_eq!(arn_patterns, &["arn:${Partition}:s3:::${BucketName}"]);
    }

    // -----------------------------------------------------------------------
    // substitute_arn_patterns tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_substitute_s3_bucket_arn() {
        let mut resource_map = ResolvedResourceMap::new();
        resource_map
            .entry(("s3".to_string(), "bucket".to_string()))
            .or_default()
            .push(make_resolved_resource(
                "aws_s3_bucket",
                "b",
                "s3",
                "bucket",
                Some("my-app-data"),
                None,
                Some("arn:${Partition}:s3:::my-app-data"),
                None
            ));
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);

        let patterns = vec!["arn:${Partition}:s3:::${BucketName}".to_string()];
        let result = resolver
            .substitute_arn_patterns("s3", "bucket", &patterns)
            .unwrap();
        assert_eq!(result, vec!["arn:${Partition}:s3:::my-app-data"]);
    }

    #[test]
    fn test_state_arns_take_precedence() {
        let mut resource_map = ResolvedResourceMap::new();
        resource_map
            .entry(("s3".to_string(), "bucket".to_string()))
            .or_default()
            .push(make_resolved_resource(
                "aws_s3_bucket",
                "b",
                "s3",
                "bucket",
                Some("hcl-bucket"),
                Some("arn:aws:s3:::state-bucket"),
                Some("arn:${Partition}:s3:::hcl-bucket"),
                None
            ));
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);

        let patterns = vec!["arn:${Partition}:s3:::${BucketName}".to_string()];
        let result = resolver
            .substitute_arn_patterns("s3", "bucket", &patterns)
            .unwrap();
        assert_eq!(result, vec!["arn:aws:s3:::state-bucket"]);
    }

    #[test]
    fn test_sub_resource_falls_back_to_parent() {
        let mut resource_map = ResolvedResourceMap::new();
        resource_map
            .entry(("s3".to_string(), "bucket".to_string()))
            .or_default()
            .push(make_resolved_resource(
                "aws_s3_bucket",
                "b",
                "s3",
                "bucket",
                Some("my-bucket"),
                None,
                Some("arn:${Partition}:s3:::my-bucket"),
                None
            ));
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);

        // Object ARN has BucketName as first placeholder → should resolve via bucket binding
        let patterns = vec!["arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()];
        let result = resolver
            .substitute_arn_patterns("s3", "object", &patterns)
            .unwrap();
        assert_eq!(result, vec!["arn:${Partition}:s3:::my-bucket/*"]);
    }

    #[test]
    fn test_no_bindings_returns_none() {
        let resolver = TerraformResourceResolver::from_resolved_map(ResolvedResourceMap::new());
        let patterns = vec!["arn:${Partition}:s3:::${BucketName}".to_string()];
        assert!(resolver
            .substitute_arn_patterns("s3", "bucket", &patterns)
            .is_none());
    }

    #[test]
    fn test_wildcard_only_binding_returns_none() {
        let mut resource_map = ResolvedResourceMap::new();
        resource_map
            .entry(("s3".to_string(), "bucket".to_string()))
            .or_default()
            .push(make_resolved_resource(
                "aws_s3_bucket",
                "d",
                "s3",
                "bucket",
                Some("*"),
                None,
                None,
                None,
            ));
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);

        let patterns = vec!["arn:${Partition}:s3:::${BucketName}".to_string()];
        assert!(resolver
            .substitute_arn_patterns("s3", "bucket", &patterns)
            .is_none());
    }

    #[test]
    fn test_preserves_infra_placeholders() {
        let mut resource_map = ResolvedResourceMap::new();
        resource_map
            .entry(("dynamodb".to_string(), "table".to_string()))
            .or_default()
            .push(make_resolved_resource(
                "aws_dynamodb_table",
                "t",
                "dynamodb",
                "table",
                Some("my-table"),
                None,
                None,
                None,
            ));
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);

        let patterns =
            vec!["arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}".to_string()];
        let result = resolver
            .substitute_arn_patterns("dynamodb", "table", &patterns)
            .unwrap();
        assert_eq!(
            result,
            vec!["arn:${Partition}:dynamodb:${Region}:${Account}:table/my-table"]
        );
    }

    #[test]
    fn test_multiple_resources_produce_multiple_arns() {
        let mut resource_map = ResolvedResourceMap::new();
        let entry = resource_map
            .entry(("s3".to_string(), "bucket".to_string()))
            .or_default();
        entry.push(make_resolved_resource(
            "aws_s3_bucket",
            "a",
            "s3",
            "bucket",
            Some("bucket-a"),
            None,
            None,
            None,
        ));
        entry.push(make_resolved_resource(
            "aws_s3_bucket",
            "b",
            "s3",
            "bucket",
            Some("bucket-b"),
            None,
            None,
            None,
        ));
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);

        let patterns = vec!["arn:${Partition}:s3:::${BucketName}".to_string()];
        let result = resolver
            .substitute_arn_patterns("s3", "bucket", &patterns)
            .unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"arn:${Partition}:s3:::bucket-a".to_string()));
        assert!(result.contains(&"arn:${Partition}:s3:::bucket-b".to_string()));
    }

    // -----------------------------------------------------------------------
    // build_binding_explanations tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_binding_explanations_terraform() {
        let mut resource_map = ResolvedResourceMap::new();
        resource_map
            .entry(("s3".to_string(), "bucket".to_string()))
            .or_default()
            .push(make_resolved_resource(
                "aws_s3_bucket",
                "data",
                "s3",
                "bucket",
                Some("my-bucket"),
                None,
                Some("arn:${Partition}:s3:::my-bucket"),
                None,
            ));
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);

        let explanations = resolver.build_binding_explanations();
        assert_eq!(explanations.len(), 1);
        assert_eq!(explanations[0].arn, "arn:${Partition}:s3:::my-bucket");
        assert_eq!(explanations[0].source, BindingSource::Terraform);
        assert_eq!(explanations[0].resource_type, "aws_s3_bucket");
        assert_eq!(explanations[0].resource_name, "data");
        assert_eq!(
            explanations[0].location,
            Location::new(PathBuf::from("main.tf"), (10, 1), (10, 1))
        );
    }

    #[test]
    fn test_build_binding_explanations_state_takes_over() {
        let mut resource_map = ResolvedResourceMap::new();
        resource_map
            .entry(("s3".to_string(), "bucket".to_string()))
            .or_default()
            .push(make_resolved_resource(
                "aws_s3_bucket",
                "data",
                "s3",
                "bucket",
                Some("my-bucket"),
                Some("arn:aws:s3:::my-bucket"),
                Some("arn:${Partition}:s3:::my-bucket"),
                Some(Location::new(PathBuf::from("terraform.tfstate"), (1, 1), (1, 1)))
            ));
        let tfstate_path = PathBuf::from("terraform.tfstate");
        let resolver =
            TerraformResourceResolver::from_resolved_map(resource_map);

        let explanations = resolver.build_binding_explanations();
        // State ARN present → Terraform explanation is suppressed, only state explanation emitted
        assert_eq!(explanations.len(), 1);
        assert_eq!(explanations[0].arn, "arn:aws:s3:::my-bucket");
        assert_eq!(explanations[0].source, BindingSource::TerraformState);
        assert_eq!(
            explanations[0].location,
            Location::new(tfstate_path, (1, 1), (1, 1))
        );
    }

    #[test]
    fn test_build_binding_explanations_empty() {
        let resolver = TerraformResourceResolver::from_resolved_map(ResolvedResourceMap::new());
        assert!(resolver.build_binding_explanations().is_empty());
    }

    // -----------------------------------------------------------------------
    // Helper function tests (parameterized)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("s3:GetObject", "s3")]
    #[case("dynamodb:PutItem", "dynamodb")]
    #[case("nocolon", "nocolon")]
    fn test_action_service(#[case] action_name: &str, #[case] expected: &str) {
        let action = Action::new(action_name.to_string(), vec![], vec![], Explanation::default());
        assert_eq!(action.service(), expected);
    }

    #[rstest]
    #[case("BucketName",   &["bucket_name", "bucket", "name"])]
    #[case("FunctionName", &["function_name", "function", "name"])]
    #[case("TableName",    &["table_name", "table", "name"])]
    fn test_placeholder_to_attribute_candidates(
        #[case] placeholder: &str,
        #[case] must_contain: &[&str],
    ) {
        let candidates = placeholder_to_attribute_candidates(placeholder);
        for expected in must_contain {
            assert!(
                candidates.contains(&expected.to_string()),
                "'{placeholder}' candidates should contain '{expected}', got: {candidates:?}"
            );
        }
    }

    #[rstest]
    #[case("s3_arn",      &["arn:${Partition}:s3:::${BucketName}"],                                      &["bucket"],  Some("bucket"))]
    #[case("dynamodb_arn", &["arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}"],        &["name"],    Some("name"))]
    fn test_derive_naming_attribute(
        #[case] _name: &str,
        #[case] patterns: &[&str],
        #[case] attr_keys: &[&str],
        #[case] expected: Option<&str>,
    ) {
        let pattern_strings: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        let attrs: HashMap<String, AttributeValue> = attr_keys
            .iter()
            .map(|k| (k.to_string(), AttributeValue::Literal("x".to_string())))
            .collect();
        assert_eq!(
            derive_naming_attribute(&pattern_strings, &attrs),
            expected.map(String::from),
        );
    }

    #[rstest]
    #[case("single_resource", "arn:${Partition}:s3:::${BucketName}", false)]
    #[case(
        "double_resource",
        "arn:${Partition}:s3:::${BucketName}/${ObjectName}",
        true
    )]
    #[case(
        "infra_not_counted",
        "arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}",
        false
    )]
    fn test_multi_placeholder_detection(
        #[case] _name: &str,
        #[case] pattern: &str,
        #[case] expected: bool,
    ) {
        let patterns = vec![pattern.to_string()];
        assert_eq!(
            arn_patterns_have_multiple_resource_placeholders(&patterns),
            expected,
            "multi-placeholder detection mismatch for '{_name}'"
        );
    }

    #[test]
    fn test_resolver_empty() {
        let resolver = TerraformResourceResolver::from_resolved_map(ResolvedResourceMap::new());
        assert!(resolver.is_empty());
        assert_eq!(resolver.len(), 0);
    }

    #[test]
    fn test_resolver_not_empty() {
        let mut resource_map = ResolvedResourceMap::new();
        resource_map
            .entry(("s3".to_string(), "bucket".to_string()))
            .or_default()
            .push(make_resolved_resource(
                "aws_s3_bucket",
                "b",
                "s3",
                "bucket",
                Some("x"),
                None,
                None,
                None,
            ));
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);
        assert!(!resolver.is_empty());
        assert_eq!(resolver.len(), 1);
    }
}
