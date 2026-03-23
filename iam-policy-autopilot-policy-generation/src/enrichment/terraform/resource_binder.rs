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
pub(crate) struct ResolvedTerraformResource {
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
}

/// Map key: `(service_name, resource_type)` e.g. `("s3", "bucket")`.
pub(crate) type ResolvedResourceKey = (String, String);

/// Indexed map of resolved resources keyed by `(service_name, resource_type)`.
/// Multiple terraform resources can map to the same IAM key (e.g. two S3 buckets).
pub(crate) type ResolvedResourceMap = HashMap<ResolvedResourceKey, Vec<ResolvedTerraformResource>>;

impl ResolvedTerraformResource {
    /// Build from a parsed resource with no enrichment.
    #[must_use]
    pub(crate) fn from_parsed(resource: TerraformResource) -> Self {
        Self {
            resource,
            service_name: None,
            resource_type: None,
            state_arn: None,
            state_arn_location: None,
            hcl_arn: None,
            binding_name: None
        }
    }

    /// The best available ARN: state ARN if present, otherwise HCL-derived.  
    #[must_use]
    #[cfg(test)]
    pub(crate) fn effective_arn(&self) -> Option<&str> {
        self.state_arn.as_deref().or(self.hcl_arn.as_deref())
    }

    /// Whether this resource has any ARN (state or HCL).  
    #[must_use]
    #[cfg(test)]
    pub(crate) fn has_arn(&self) -> bool {
        self.effective_arn().is_some()
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
pub(crate) struct TerraformResourceResolver {
    /// All resolved resources keyed by `(service_name, resource_type)`.
    resources: ResolvedResourceMap,
    /// Parse warnings from HCL parsing.
    #[allow(dead_code)]
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
    pub(crate) async fn new(
        terraform_dir: Option<&Path>,
        terraform_files: &[PathBuf],
        tfstate_paths: &[PathBuf],
        tfvars_paths: &[PathBuf],
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

        // Step 2: Resolve variables
        // When a directory is provided: extract defaults + auto-discover tfvars + explicit tfvars
        // When only terraform files are given with explicit tfvars: apply tfvars directly
        // When neither dir nor tfvars: no variable resolution
        let var_ctx = if let Some(directory) = terraform_dir {
            VariableContext::from_directory_with_explicit_tfvars(directory, tfvars_paths)
                .unwrap_or_else(|e| {
                    log::warn!("Failed to resolve Terraform variables: {e}");
                    VariableContext::default()
                })
        } else if !tfvars_paths.is_empty() && !terraform_files.is_empty() {
            VariableContext::from_explicit_tfvars(tfvars_paths)
        } else {
            VariableContext::default()
        };
        var_ctx.resolve_attributes(&mut tf_result);

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

    /// Build a resolver directly from pre-computed components (useful for testing).
    #[cfg(test)]
    pub(crate) fn from_resolved_map(resources: ResolvedResourceMap) -> Self {
        Self {
            resources,
            warnings: Vec::new(),
        }
    }

    /// Returns `true` if no IAM-mappable resources were resolved.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }


    /// Number of distinct `(service, resource_type)` groups resolved.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.resources.len()
    }

    // -----------------------------------------------------------------------
    // ARN substitution
    // -----------------------------------------------------------------------


    /// Apply Terraform resource bindings to enriched SDK calls.
    ///
    /// For each action's resources, if the resource type has a matching binding,
    /// substitute the ARN patterns with concrete values. Otherwise, leave unchanged.
    pub(crate) fn substitute_enriched_calls<'a>(
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
            if let Some((naming_attr, matched_pattern)) =
                derive_naming_attribute(&patterns, &resource.attributes)
            {
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
                        let re = placeholder_regex();
                        let mut replaced = false;
                        let hcl_arn = re
                            .replace_all(&matched_pattern, |caps: &regex::Captures| {
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
/// that exists in the resource's attributes, along with the ARN pattern that
/// matched.
///
/// Returns `None` if no matching attribute is found — callers should treat this
/// as "unknown" and skip the resource rather than guessing.
pub(crate) fn derive_naming_attribute(
    arn_patterns: &[String],
    attributes: &HashMap<String, AttributeValue>,
) -> Option<(String, String)> {
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
                    return Some((candidate.clone(), pattern.clone()));
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
            binding_name: binding_name.map(String::from)
        }
    }

    // -----------------------------------------------------------------------
    // ResolvedTerraformResource tests (parameterized)
    // -----------------------------------------------------------------------

    #[rstest]
    // Freshly parsed: no service mapping, no ARN
    #[case("from_parsed",       None,                                   None,                                   false, false, None)]
    // State ARN takes precedence over HCL ARN
    #[case("prefers_state",     Some("arn:${Partition}:s3:::x"),         Some("arn:aws:s3:::real-bucket"),        true,  true,  Some("arn:aws:s3:::real-bucket"))]
    // HCL ARN used when no state ARN
    #[case("falls_back_to_hcl", Some("arn:${Partition}:s3:::x"),         None,                                   true,  false, Some("arn:${Partition}:s3:::x"))]
    fn test_resolved_resource_properties(
        #[case] _name: &str,
        #[case] hcl_arn: Option<&str>,
        #[case] state_arn: Option<&str>,
        #[case] has_arn: bool,
        #[case] has_state: bool,
        #[case] effective: Option<&str>,
    ) {
        let mut r = ResolvedTerraformResource::from_parsed(make_hcl_resource(
            "aws_s3_bucket", "b", "bucket", AttributeValue::Literal("x".into()),
        ));
        r.hcl_arn = hcl_arn.map(String::from);
        r.state_arn = state_arn.map(String::from);
        assert_eq!(r.has_arn(), has_arn, "[{_name}] has_arn");
        assert_eq!(r.state_arn.is_some(), has_state, "[{_name}] has_state");
        assert_eq!(r.effective_arn(), effective, "[{_name}] effective_arn");
    }

    // -----------------------------------------------------------------------
    // resolve_terraform_resources tests (shared harness)
    // -----------------------------------------------------------------------

    /// Shared harness for resolve_terraform_resources tests.
    async fn assert_resolve(
        hcl: Vec<TerraformResource>,
        state_entries: Vec<(&str, &str, Option<&str>)>,
        expected_key: Option<(&str, &str)>,
        expected_count: Option<usize>,
        expected_state_arn: Option<Option<&str>>,
        expected_service: Option<&str>,
    ) {
        let state = make_state_resources(state_entries);
        let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
        let resolved = resolve_terraform_resources(
            &vec_to_terraform_resources(hcl),
            &state,
            &empty_var_ctx(),
            &loader,
        )
        .await;

        match expected_key {
            None => assert!(resolved.is_empty(), "expected empty resolved map"),
            Some((svc, rtype)) => {
                let key = (svc.to_string(), rtype.to_string());
                assert!(resolved.contains_key(&key), "key {svc}/{rtype} not found");
                if let Some(count) = expected_count {
                    assert_eq!(resolved[&key].len(), count, "count mismatch for {svc}/{rtype}");
                }
                if let Some(arn) = expected_state_arn {
                    assert_eq!(resolved[&key][0].state_arn.as_deref(), arn, "state_arn mismatch");
                }
                if let Some(service) = expected_service {
                    assert_eq!(resolved[&key][0].service_name.as_deref(), Some(service), "service mismatch");
                }
            }
        }
    }

    #[rstest]
    // State ARN attached when matching by type + name
    #[case(
        "attaches_state_arn",
        vec![("aws_s3_bucket", "data", "bucket", "my-bucket")],
        vec![("aws_s3_bucket", "data", Some("arn:aws:s3:::my-bucket"))],
        Some(("s3", "bucket")), Some(1), Some(Some("arn:aws:s3:::my-bucket")), None
    )]
    // No state match → state_arn is None
    #[case(
        "no_state_match",
        vec![("aws_s3_bucket", "data", "bucket", "x")],
        vec![("aws_s3_bucket", "other", Some("arn:aws:s3:::other"))],
        Some(("s3", "bucket")), Some(1), Some(None), None
    )]
    // IAM mapping resolves service name
    #[case(
        "iam_mapping",
        vec![("aws_s3_bucket", "data", "bucket", "x")],
        vec![],
        Some(("s3", "bucket")), None, None, Some("s3")
    )]
    // Empty inputs → empty result
    #[case(
        "empty_inputs",
        vec![],
        vec![],
        None, None, None, None
    )]
    // Non-AWS resources excluded
    #[case(
        "non_aws_excluded",
        vec![("null_resource", "x", "name", "y")],
        vec![],
        None, None, None, None
    )]
    // Multiple resources of same IAM type grouped together
    #[case(
        "multiple_same_type",
        vec![("aws_s3_bucket", "a", "bucket", "bucket-a"), ("aws_s3_bucket", "b", "bucket", "bucket-b")],
        vec![],
        Some(("s3", "bucket")), Some(2), None, None
    )]
    #[tokio::test]
    async fn test_resolve_terraform_resources(
        #[case] _name: &str,
        #[case] hcl_input: Vec<(&str, &str, &str, &str)>,
        #[case] state_entries: Vec<(&str, &str, Option<&str>)>,
        #[case] expected_key: Option<(&str, &str)>,
        #[case] expected_count: Option<usize>,
        #[case] expected_state_arn: Option<Option<&str>>,
        #[case] expected_service: Option<&str>,
    ) {
        let hcl: Vec<TerraformResource> = hcl_input
            .into_iter()
            .map(|(rtype, name, attr_key, attr_val)| {
                make_hcl_resource(rtype, name, attr_key, AttributeValue::Literal(attr_val.into()))
            })
            .collect();
        assert_resolve(hcl, state_entries, expected_key, expected_count, expected_state_arn, expected_service).await;
    }

    // -----------------------------------------------------------------------
    // substitute_enriched_calls tests (shared harness)
    // -----------------------------------------------------------------------

    /// Shared harness for substitute_enriched_calls tests.
    ///
    /// Builds an `EnrichedSdkMethodCall` for s3:GetObject with the given ARN
    /// patterns, resolves it against the provided resource map entries, and
    /// asserts the resulting ARN patterns match `expected_arns`.
    fn assert_substitute_enriched_calls(
        resources: &[(&str, &str, &str, &str, Option<&str>, Option<&str>, Option<&str>)],
        expected_arns: &[&str],
    ) {
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
        for &(rtype, local_name, service, suffix, binding_name, state_arn, hcl_arn) in resources {
            resource_map
                .entry((service.to_string(), suffix.to_string()))
                .or_default()
                .push(make_resolved_resource(
                    rtype,
                    local_name,
                    service,
                    suffix,
                    binding_name,
                    state_arn,
                    hcl_arn,
                    None,
                ));
        }
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);
        let result = resolver.substitute_enriched_calls(&enriched);

        assert_eq!(result.len(), 1);
        let arn_patterns = result[0].actions[0].resources[0]
            .arn_patterns
            .as_ref()
            .unwrap();
        let expected: Vec<String> = expected_arns.iter().map(|s| s.to_string()).collect();
        assert_eq!(arn_patterns, &expected);
    }

    #[rstest]
    #[case(
        "replaces_arns",
        &[("aws_s3_bucket", "data", "s3", "bucket", Some("my-app-data"), None, Some("arn:${Partition}:s3:::my-app-data"))],
        &["arn:${Partition}:s3:::my-app-data"]
    )]
    #[case(
        "no_match_keeps_original",
        &[],
        &["arn:${Partition}:s3:::${BucketName}"]
    )]
    fn test_substitute_enriched_calls(
        #[case] _name: &str,
        #[case] resources: &[(&str, &str, &str, &str, Option<&str>, Option<&str>, Option<&str>)],
        #[case] expected_arns: &[&str],
    ) {
        assert_substitute_enriched_calls(resources, expected_arns);
    }

    // -----------------------------------------------------------------------
    // substitute_arn_patterns tests (shared harness)
    // -----------------------------------------------------------------------

    /// Shared harness for substitute_arn_patterns tests.
    ///
    /// Builds a `TerraformResourceResolver` from the given resource entries,
    /// calls `substitute_arn_patterns`, and compares the result against expected.
    /// For multi-resource cases (where order is non-deterministic), the results
    /// are sorted before comparison.
    fn assert_substitute_arn_patterns(
        resources: &[(&str, &str, &str, &str, Option<&str>, Option<&str>, Option<&str>)],
        query_service: &str,
        query_resource_type: &str,
        patterns: &[&str],
        expected: Option<&[&str]>,
    ) {
        let mut resource_map = ResolvedResourceMap::new();
        for &(rtype, local_name, service, suffix, binding_name, state_arn, hcl_arn) in resources {
            resource_map
                .entry((service.to_string(), suffix.to_string()))
                .or_default()
                .push(make_resolved_resource(
                    rtype,
                    local_name,
                    service,
                    suffix,
                    binding_name,
                    state_arn,
                    hcl_arn,
                    None,
                ));
        }
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);

        let pattern_strings: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        let result = resolver.substitute_arn_patterns(query_service, query_resource_type, &pattern_strings);

        match expected {
            None => assert!(result.is_none(), "expected None but got {result:?}"),
            Some(exp) => {
                let mut actual = result.expect("expected Some but got None");
                actual.sort();
                let mut expected_sorted: Vec<String> = exp.iter().map(|s| s.to_string()).collect();
                expected_sorted.sort();
                assert_eq!(actual, expected_sorted);
            }
        }
    }

    #[rstest]
    // S3 bucket ARN resolved from HCL
    #[case(
        "s3_bucket_arn",
        &[("aws_s3_bucket", "b", "s3", "bucket", Some("my-app-data"), None, Some("arn:${Partition}:s3:::my-app-data"))],
        "s3", "bucket",
        &["arn:${Partition}:s3:::${BucketName}"],
        Some(&["arn:${Partition}:s3:::my-app-data"] as &[&str])
    )]
    // State ARN takes precedence over HCL ARN
    #[case(
        "state_arns_take_precedence",
        &[("aws_s3_bucket", "b", "s3", "bucket", Some("hcl-bucket"), Some("arn:aws:s3:::state-bucket"), Some("arn:${Partition}:s3:::hcl-bucket"))],
        "s3", "bucket",
        &["arn:${Partition}:s3:::${BucketName}"],
        Some(&["arn:aws:s3:::state-bucket"] as &[&str])
    )]
    // Sub-resource (object) falls back to parent resource (bucket) binding
    #[case(
        "sub_resource_falls_back_to_parent",
        &[("aws_s3_bucket", "b", "s3", "bucket", Some("my-bucket"), None, Some("arn:${Partition}:s3:::my-bucket"))],
        "s3", "object",
        &["arn:${Partition}:s3:::${BucketName}/${ObjectName}"],
        Some(&["arn:${Partition}:s3:::my-bucket/*"] as &[&str])
    )]
    // No bindings → None
    #[case(
        "no_bindings_returns_none",
        &[],
        "s3", "bucket",
        &["arn:${Partition}:s3:::${BucketName}"],
        None
    )]
    // Wildcard-only binding → None
    #[case(
        "wildcard_only_returns_none",
        &[("aws_s3_bucket", "d", "s3", "bucket", Some("*"), None, None)],
        "s3", "bucket",
        &["arn:${Partition}:s3:::${BucketName}"],
        None
    )]
    // Infrastructure placeholders (Region, Account, Partition) preserved
    #[case(
        "preserves_infra_placeholders",
        &[("aws_dynamodb_table", "t", "dynamodb", "table", Some("my-table"), None, None)],
        "dynamodb", "table",
        &["arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}"],
        Some(&["arn:${Partition}:dynamodb:${Region}:${Account}:table/my-table"] as &[&str])
    )]
    // Multiple resources produce multiple ARNs
    #[case(
        "multiple_resources_produce_multiple_arns",
        &[("aws_s3_bucket", "a", "s3", "bucket", Some("bucket-a"), None, None),
          ("aws_s3_bucket", "b", "s3", "bucket", Some("bucket-b"), None, None)],
        "s3", "bucket",
        &["arn:${Partition}:s3:::${BucketName}"],
        Some(&["arn:${Partition}:s3:::bucket-a", "arn:${Partition}:s3:::bucket-b"] as &[&str])
    )]
    fn test_substitute_arn_patterns(
        #[case] _name: &str,
        #[case] resources: &[(&str, &str, &str, &str, Option<&str>, Option<&str>, Option<&str>)],
        #[case] query_service: &str,
        #[case] query_resource_type: &str,
        #[case] patterns: &[&str],
        #[case] expected: Option<&[&str]>,
    ) {
        assert_substitute_arn_patterns(resources, query_service, query_resource_type, patterns, expected);
    }

    // -----------------------------------------------------------------------
    // build_binding_explanations tests (parameterized)
    // -----------------------------------------------------------------------

    #[rstest]
    // Terraform source → explanation with HCL location
    #[case(
        "terraform_source",
        vec![("aws_s3_bucket", "data", "s3", "bucket", Some("my-bucket"), None, Some("arn:${Partition}:s3:::my-bucket"), false)],
        vec![("arn:${Partition}:s3:::my-bucket", BindingSource::Terraform, "aws_s3_bucket", "data", "main.tf")]
    )]
    // State takes precedence → explanation with state location
    #[case(
        "state_takes_precedence",
        vec![("aws_s3_bucket", "data", "s3", "bucket", Some("my-bucket"), Some("arn:aws:s3:::my-bucket"), Some("arn:${Partition}:s3:::my-bucket"), true)],
        vec![("arn:aws:s3:::my-bucket", BindingSource::TerraformState, "aws_s3_bucket", "data", "terraform.tfstate")]
    )]
    // Empty → no explanations
    #[case(
        "empty",
        vec![],
        vec![]
    )]
    fn test_binding_explanations(
        #[case] _name: &str,
        #[case] resources: Vec<(&str, &str, &str, &str, Option<&str>, Option<&str>, Option<&str>, bool)>,
        #[case] expected: Vec<(&str, BindingSource, &str, &str, &str)>,
    ) {
        let mut resource_map = ResolvedResourceMap::new();
        for (rtype, local_name, service, suffix, binding_name, state_arn, hcl_arn, has_state_loc) in &resources {
            let state_loc = if *has_state_loc {
                Some(Location::new(PathBuf::from("terraform.tfstate"), (1, 1), (1, 1)))
            } else {
                None
            };
            resource_map
                .entry((service.to_string(), suffix.to_string()))
                .or_default()
                .push(make_resolved_resource(
                    rtype, local_name, service, suffix,
                    *binding_name, *state_arn, *hcl_arn, state_loc,
                ));
        }
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);
        let explanations = resolver.build_binding_explanations();
        assert_eq!(explanations.len(), expected.len(), "count mismatch");
        for (actual, (exp_arn, exp_source, exp_rtype, exp_rname, exp_file)) in explanations.iter().zip(expected.iter()) {
            assert_eq!(actual.arn, *exp_arn, "arn mismatch");
            assert_eq!(actual.source, *exp_source, "source mismatch");
            assert_eq!(actual.resource_type, *exp_rtype, "resource_type mismatch");
            assert_eq!(actual.resource_name, *exp_rname, "resource_name mismatch");
            assert_eq!(actual.location.file_path, PathBuf::from(exp_file), "location file mismatch");
        }
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
    #[case("s3_arn",       &["arn:${Partition}:s3:::${BucketName}"],                                      &["bucket"],  Some(("bucket", "arn:${Partition}:s3:::${BucketName}")))]
    #[case("dynamodb_arn", &["arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}"],         &["name"],    Some(("name",   "arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}")))]
    #[case("match_second_pattern_different_resource",
        &["arn:${Partition}:iam::${Account}:role/${RoleName}",
          "arn:${Partition}:iam::${Account}:policy/${PolicyName}"],
        &["policy_name"],
        Some(("policy_name", "arn:${Partition}:iam::${Account}:policy/${PolicyName}")))]
    #[case("match_second_pattern_deeper_arn",
        &["arn:${Partition}:kinesis:${Region}:${Account}:stream/${StreamName}",
          "arn:${Partition}:kinesis:${Region}:${Account}:stream/${StreamName}/consumer/${ConsumerName}"],
        &["consumer_name"],
        Some(("consumer_name", "arn:${Partition}:kinesis:${Region}:${Account}:stream/${StreamName}/consumer/${ConsumerName}")))]
    fn test_derive_naming_attribute(
        #[case] _name: &str,
        #[case] patterns: &[&str],
        #[case] attr_keys: &[&str],
        #[case] expected: Option<(&str, &str)>,
    ) {
        let pattern_strings: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        let attrs: HashMap<String, AttributeValue> = attr_keys
            .iter()
            .map(|k| (k.to_string(), AttributeValue::Literal("x".to_string())))
            .collect();
        assert_eq!(
            derive_naming_attribute(&pattern_strings, &attrs),
            expected.map(|(attr, pat)| (String::from(attr), String::from(pat))),
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

    #[rstest]
    #[case("empty",     &[],                                                                 true,  0)]
    #[case("non_empty", &[("aws_s3_bucket", "b", "s3", "bucket", Some("x"), None, None)],    false, 1)]
    fn test_resolver_len_and_empty(
        #[case] _name: &str,
        #[case] resources: &[(&str, &str, &str, &str, Option<&str>, Option<&str>, Option<&str>)],
        #[case] expected_empty: bool,
        #[case] expected_len: usize,
    ) {
        let mut resource_map = ResolvedResourceMap::new();
        for &(rtype, local_name, service, suffix, binding_name, state_arn, hcl_arn) in resources {
            resource_map
                .entry((service.to_string(), suffix.to_string()))
                .or_default()
                .push(make_resolved_resource(rtype, local_name, service, suffix, binding_name, state_arn, hcl_arn, None));
        }
        let resolver = TerraformResourceResolver::from_resolved_map(resource_map);
        assert_eq!(resolver.is_empty(), expected_empty, "[{_name}] is_empty");
        assert_eq!(resolver.len(), expected_len, "[{_name}] len");
    }
}
