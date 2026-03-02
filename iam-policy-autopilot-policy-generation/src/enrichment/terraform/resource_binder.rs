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

use crate::enrichment::{Action, EnrichedSdkMethodCall, Resource};
use crate::enrichment::ServiceReferenceLoader;

use crate::extraction::terraform::hcl_parser::parse_terraform_directory;
use crate::extraction::terraform::state_parser::{parse_terraform_state, StateResourceMap};
use crate::extraction::terraform::variable_resolver::VariableContext;
use crate::extraction::terraform::{AttributeValue, TerraformResource};

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
    /// The original parsed resource.
    pub resource: TerraformResource,
    /// IAM service name derived from `names_data.hcl` (e.g. `"s3"`).
    pub service_name: Option<String>,
    /// IAM resource suffix derived from `names_data.hcl` (e.g. `"bucket"`).
    pub resource_type: Option<String>,
    /// Concrete ARN from `terraform.tfstate`. Takes precedence over `hcl_arn`.
    pub state_arn: Option<String>,
    /// ARN constructed from HCL attributes + SDF patterns. Fallback when `state_arn` is absent.
    pub hcl_arn: Option<String>,
    /// The resolved naming attribute value (e.g. `"my-app-data-bucket"`).
    pub binding_name: Option<String>,
    /// Source files traced from this resource (e.g. Lambda handler code).
    pub traced_sources: Vec<PathBuf>,
    /// Handler entry point if applicable (e.g. `"index.handler"`).
    pub handler: Option<String>,
    /// Human-readable source location string (e.g. `"main.tf:12"`).
    pub location: String,
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
        let location = match resource.line_number {
            Some(line) => format!("{}:{}", resource.source_file.display(), line),
            None => resource.source_file.display().to_string(),
        };
        Self {
            resource,
            service_name: None,
            resource_type: None,
            state_arn: None,
            hcl_arn: None,
            binding_name: None,
            traced_sources: Vec::new(),
            handler: None,
            location,
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
/// Owns the full lifecycle: parsing HCL, resolving variables, tracing sources,
/// loading state, mapping to IAM services, deriving ARNs, and substituting
/// concrete resource names into enriched SDK calls.
#[derive(Debug)]
pub struct TerraformResourceResolver {
    /// All resolved resources keyed by `(service_name, resource_type)`.
    resources: ResolvedResourceMap,
    /// Parse warnings from HCL parsing.
    warnings: Vec<String>,
    /// Path to the `terraform.tfstate` file, if provided.
    tfstate_path: Option<PathBuf>,
}

impl TerraformResourceResolver {
    /// Build a resolver from a Terraform project directory.
    ///
    /// This is the main factory method. It:
    /// 1. Parses `.tf` files in the directory
    /// 2. Resolves `var.xxx` references from defaults + `.tfvars` files
    /// 3. Traces source code from compute resources (e.g. Lambda handlers)
    /// 4. Optionally parses `terraform.tfstate` for deployed ARNs
    /// 5. Resolves each HCL resource to its service, ARN, and metadata
    pub async fn from_directory(
        directory: &Path,
        tfstate_path: Option<&PathBuf>,
        loader: &ServiceReferenceLoader,
    ) -> Result<Self> {
        // Step 1: Parse Terraform HCL files
        let mut tf_result =
            parse_terraform_directory(directory).context("Failed to parse Terraform directory")?;

        for warning in &tf_result.warnings {
            log::warn!("Terraform parse warning: {warning}");
        }

        // Step 2: Resolve variables
        let var_ctx = VariableContext::from_directory(directory).unwrap_or_else(|e| {
            log::warn!("Failed to resolve Terraform variables: {e}");
            VariableContext::default()
        });
        var_ctx.resolve_attributes(&mut tf_result);

        debug!(
            "Parsed {} Terraform resources",
            tf_result.resources.len(),
        );

        // Step 3: Parse terraform.tfstate if provided
        let state_map = if let Some(state_path) = tfstate_path {
            debug!("Loading Terraform state from {}", state_path.display());
            let map =
                parse_terraform_state(state_path).context("Failed to parse terraform.tfstate")?;
            debug!("Parsed {} state resource groups", map.len());
            map
        } else {
            StateResourceMap::new()
        };

        // Step 4: Resolve all resources
        let resources = resolve_terraform_resources(
            &tf_result.resources,
            &state_map,
            &var_ctx,
            loader,
        )
        .await;

        debug!(
            "Resolved {} resource groups from Terraform",
            resources.len()
        );

        Ok(Self {
            resources,
            warnings: tf_result.warnings,
            tfstate_path: tfstate_path.cloned(),
        })
    }

    /// Build a resolver directly from pre-computed components (useful for testing).
    #[cfg(test)]
    pub fn from_parts(resources: ResolvedResourceMap, tfstate_path: Option<PathBuf>) -> Self {
        Self {
            resources,
            warnings: Vec::new(),
            tfstate_path,
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
                        let service = extract_service_from_action(&action.name);
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
                if is_sub_resource
                    && self
                        .lookup_full_arns(service, resource_type_name)
                        .is_none()
                {
                    let sub_arns: Vec<String> =
                        full_arns.iter().map(|a| format!("{a}/*")).collect();
                    return Some(sub_arns);
                }
                return Some(full_arns.to_vec());
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
    /// Uses the pre-computed `hcl_arn`, `state_arn`, and `location` fields
    /// on each `ResolvedTerraformResource`, avoiding re-derivation.
    #[must_use]
    pub fn build_binding_explanations(&self) -> Vec<ResourceBindingExplanation> {
        let mut explanations = Vec::new();

        for resources in self.resources.values() {
            for resolved in resources {
                // HCL-derived ARN explanation
                if let Some(ref hcl_arn) = resolved.hcl_arn {
                    // Skip if state ARN will also be emitted (state takes precedence)
                    if resolved.state_arn.is_none() {
                        explanations.push(ResourceBindingExplanation {
                            arn: hcl_arn.clone(),
                            source: BindingSource::Hcl,
                            terraform_resource_type: resolved.resource.resource_type.clone(),
                            terraform_resource_name: resolved.resource.local_name.clone(),
                            location: resolved.location.clone(),
                        });
                    }
                }

                // State-derived ARN explanation
                if let Some(ref state_arn) = resolved.state_arn {
                    let location = self
                        .tfstate_path
                        .as_ref()
                        .map_or_else(|| resolved.location.clone(), |p| p.display().to_string());

                    explanations.push(ResourceBindingExplanation {
                        arn: state_arn.clone(),
                        source: BindingSource::TerraformState,
                        terraform_resource_type: resolved.resource.resource_type.clone(),
                        terraform_resource_name: resolved.resource.local_name.clone(),
                        location,
                    });
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
/// policy generation. Resources that don't map to an IAM service are excluded.
async fn resolve_terraform_resources(
    hcl_resources: &crate::extraction::terraform::TerraformResourceMap,
    state_map: &StateResourceMap,
    var_ctx: &VariableContext,
    loader: &ServiceReferenceLoader,
) -> ResolvedResourceMap {
    let resolver = super::service_resolver::TerraformServiceAndResourceResolver::global();
    let mut results = ResolvedResourceMap::new();

    for resource in hcl_resources.values() {
        let mut resolved = ResolvedTerraformResource::from_parsed(resource.clone());
        let tf_key = (resource.resource_type.clone(), resource.local_name.clone());

        let Some((service, suffix)) = resolver.resolve(&resource.resource_type) else {
            continue;
        };
        resolved.service_name = Some(service.clone());
        resolved.resource_type = Some(suffix.clone());

        // Derive HCL ARN from SDF patterns + naming attribute
        if let Some(patterns) = loader
            .get_resource_arns(&service, &suffix)
            .await
        {
            if let Some(naming_attr) = derive_naming_attribute(&patterns, &resource.attributes) {
                if let Some(attr_value) = resource.attributes.get(&naming_attr) {
                    let resolved_value = match attr_value {
                        AttributeValue::Literal(s) => Some(s.clone()),
                        AttributeValue::Expression(_) => var_ctx
                            .try_resolve(attr_value)
                            .and_then(|v| match v {
                                AttributeValue::Literal(s) => Some(s),
                                _ => None,
                            }),
                    };
                    if let Some(name) = resolved_value {
                        resolved.binding_name = Some(name.clone());
                        if let Some(pattern) = patterns.first() {
                            let re = placeholder_regex();
                            let mut replaced = false;
                            let hcl_arn = re
                                .replace_all(pattern, |caps: &regex::Captures| {
                                    let ph = caps.get(1).map_or("", |m| m.as_str());
                                    if AWS_PLACEHOLDERS
                                        .iter()
                                        .any(|p| p.eq_ignore_ascii_case(ph))
                                    {
                                        format!("${{{ph}}}")
                                    } else if !replaced {
                                        replaced = true;
                                        name.clone()
                                    } else {
                                        "*".to_string()
                                    }
                                })
                                .to_string();
                            resolved.hcl_arn = Some(hcl_arn);
                        }
                    } else {
                        // Expression couldn't be resolved — use wildcard as binding name
                        resolved.binding_name = Some("*".to_string());
                    }
                }
            }
        }

        // Attach state ARN if available
        if let Some(state_resources) = state_map.get(&tf_key) {
            if let Some(arn) = state_resources.iter().find_map(|s| s.arn.as_ref()) {
                resolved.state_arn = Some(arn.clone());
            }
        }

        results
            .entry((service, suffix))
            .or_default()
            .push(resolved);
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

/// Extract the service name from an IAM action string (e.g., "s3" from "s3:GetObject").
fn extract_service_from_action(action_name: &str) -> &str {
    action_name.split(':').next().unwrap_or(action_name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrichment::{Action, EnrichedSdkMethodCall, Resource};
    use crate::extraction::terraform::state_parser::StateResource;
    use crate::extraction::terraform::{AttributeValue, TerraformResource};
    use super::BindingSource;
    use crate::Explanation;
    use crate::SdkMethodCall;
    use std::collections::HashMap;
    use std::path::PathBuf;

    // -----------------------------------------------------------------------
    // Test fixture helpers
    // -----------------------------------------------------------------------

    fn vec_to_resource_map(resources: Vec<TerraformResource>) -> crate::extraction::terraform::TerraformResourceMap {
        resources
            .into_iter()
            .map(|r| ((r.resource_type.clone(), r.local_name.clone()), r))
            .collect()
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
            source_file: PathBuf::from("main.tf"),
            line_number: Some(10),
        }
    }

    fn make_state_map(entries: Vec<(&str, &str, Option<&str>)>) -> StateResourceMap {
        let mut map = StateResourceMap::new();
        for (rtype, name, arn) in entries {
            map.entry((rtype.to_string(), name.to_string()))
                .or_default()
                .push(StateResource {
                    resource_type: rtype.to_string(),
                    name: name.to_string(),
                    arn: arn.map(String::from),
                });
        }
        map
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
    ) -> ResolvedTerraformResource {
        ResolvedTerraformResource {
            resource: TerraformResource {
                resource_type: rtype.to_string(),
                local_name: local_name.to_string(),
                attributes: HashMap::new(),
                source_file: PathBuf::from("main.tf"),
                line_number: Some(10),
            },
            service_name: Some(service.to_string()),
            resource_type: Some(suffix.to_string()),
            state_arn: state_arn.map(String::from),
            hcl_arn: hcl_arn.map(String::from),
            binding_name: binding_name.map(String::from),
            traced_sources: Vec::new(),
            handler: None,
            location: "main.tf:10".to_string(),
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
        assert_eq!(resolved.location, "main.tf:10");
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
        let state = make_state_map(vec![(
            "aws_s3_bucket",
            "data",
            Some("arn:aws:s3:::my-bucket"),
        )]);
        let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
        let resolved = resolve_terraform_resources(
            &vec_to_resource_map(hcl),
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
        let state = make_state_map(vec![(
            "aws_s3_bucket",
            "other",
            Some("arn:aws:s3:::other"),
        )]);
        let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
        let resolved = resolve_terraform_resources(
            &vec_to_resource_map(hcl),
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
            &vec_to_resource_map(hcl),
            &StateResourceMap::new(),
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
            &crate::extraction::terraform::TerraformResourceMap::new(),
            &StateResourceMap::new(),
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
            &vec_to_resource_map(hcl),
            &StateResourceMap::new(),
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
            &vec_to_resource_map(hcl),
            &StateResourceMap::new(),
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
            ));
        let resolver = TerraformResourceResolver::from_parts(resource_map, None);
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
        let resolver = TerraformResourceResolver::from_parts(ResolvedResourceMap::new(), None);
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
            ));
        let resolver = TerraformResourceResolver::from_parts(resource_map, None);

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
            ));
        let resolver = TerraformResourceResolver::from_parts(resource_map, None);

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
            ));
        let resolver = TerraformResourceResolver::from_parts(resource_map, None);

        // Object ARN has BucketName as first placeholder → should resolve via bucket binding
        let patterns = vec!["arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()];
        let result = resolver
            .substitute_arn_patterns("s3", "object", &patterns)
            .unwrap();
        assert_eq!(result, vec!["arn:${Partition}:s3:::my-bucket/*"]);
    }

    #[test]
    fn test_no_bindings_returns_none() {
        let resolver = TerraformResourceResolver::from_parts(ResolvedResourceMap::new(), None);
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
            ));
        let resolver = TerraformResourceResolver::from_parts(resource_map, None);

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
            ));
        let resolver = TerraformResourceResolver::from_parts(resource_map, None);

        let patterns = vec![
            "arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}".to_string(),
        ];
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
        ));
        entry.push(make_resolved_resource(
            "aws_s3_bucket",
            "b",
            "s3",
            "bucket",
            Some("bucket-b"),
            None,
            None,
        ));
        let resolver = TerraformResourceResolver::from_parts(resource_map, None);

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
    fn test_build_binding_explanations_hcl() {
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
            ));
        let resolver = TerraformResourceResolver::from_parts(resource_map, None);

        let explanations = resolver.build_binding_explanations();
        assert_eq!(explanations.len(), 1);
        assert_eq!(explanations[0].arn, "arn:${Partition}:s3:::my-bucket");
        assert_eq!(explanations[0].source, BindingSource::Hcl);
        assert_eq!(explanations[0].terraform_resource_type, "aws_s3_bucket");
        assert_eq!(explanations[0].location, "main.tf:10");
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
            ));
        let tfstate_path = PathBuf::from("terraform.tfstate");
        let resolver =
            TerraformResourceResolver::from_parts(resource_map, Some(tfstate_path.clone()));

        let explanations = resolver.build_binding_explanations();
        // State ARN present → HCL explanation is suppressed, only state explanation emitted
        assert_eq!(explanations.len(), 1);
        assert_eq!(explanations[0].arn, "arn:aws:s3:::my-bucket");
        assert_eq!(explanations[0].source, BindingSource::TerraformState);
        assert_eq!(
            explanations[0].location,
            tfstate_path.display().to_string()
        );
    }

    #[test]
    fn test_build_binding_explanations_empty() {
        let resolver = TerraformResourceResolver::from_parts(ResolvedResourceMap::new(), None);
        assert!(resolver.build_binding_explanations().is_empty());
    }

    // -----------------------------------------------------------------------
    // Helper function tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_service_from_action() {
        assert_eq!(extract_service_from_action("s3:GetObject"), "s3");
        assert_eq!(extract_service_from_action("dynamodb:PutItem"), "dynamodb");
        assert_eq!(extract_service_from_action("nocolon"), "nocolon");
    }

    #[test]
    fn test_placeholder_candidates_bucket_name() {
        let candidates = placeholder_to_attribute_candidates("BucketName");
        assert!(candidates.contains(&"bucket_name".to_string()));
        assert!(candidates.contains(&"bucket".to_string()));
    }

    #[test]
    fn test_placeholder_candidates_function_name() {
        let candidates = placeholder_to_attribute_candidates("FunctionName");
        assert!(candidates.contains(&"function_name".to_string()));
        assert!(candidates.contains(&"function".to_string()));
    }

    #[test]
    fn test_derive_naming_attribute_from_s3_arn() {
        let patterns = vec!["arn:${Partition}:s3:::${BucketName}".to_string()];
        let attrs = HashMap::from([(
            "bucket".to_string(),
            AttributeValue::Literal("x".to_string()),
        )]);
        assert_eq!(
            derive_naming_attribute(&patterns, &attrs),
            Some("bucket".to_string())
        );
    }

    #[test]
    fn test_derive_naming_attribute_from_dynamodb_arn() {
        let patterns = vec![
            "arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}".to_string(),
        ];
        let attrs = HashMap::from([(
            "name".to_string(),
            AttributeValue::Literal("x".to_string()),
        )]);
        assert_eq!(
            derive_naming_attribute(&patterns, &attrs),
            Some("name".to_string())
        );
    }

    #[test]
    fn test_multi_placeholder_detection_single() {
        let patterns = vec!["arn:${Partition}:s3:::${BucketName}".to_string()];
        assert!(!arn_patterns_have_multiple_resource_placeholders(&patterns));
    }

    #[test]
    fn test_multi_placeholder_detection_double() {
        let patterns =
            vec!["arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()];
        assert!(arn_patterns_have_multiple_resource_placeholders(&patterns));
    }

    #[test]
    fn test_multi_placeholder_detection_infra_not_counted() {
        let patterns = vec![
            "arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}".to_string(),
        ];
        assert!(!arn_patterns_have_multiple_resource_placeholders(&patterns));
    }

    #[test]
    fn test_resolver_empty() {
        let resolver = TerraformResourceResolver::from_parts(ResolvedResourceMap::new(), None);
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
            ));
        let resolver = TerraformResourceResolver::from_parts(resource_map, None);
        assert!(!resolver.is_empty());
        assert_eq!(resolver.len(), 1);
    }
}
