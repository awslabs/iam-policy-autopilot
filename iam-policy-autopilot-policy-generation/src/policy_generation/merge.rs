//! Policy merging functionality for combining multiple IAM policies
//!
//! This module provides functionality to merge multiple IAM policies into a single optimized policy
//! while preventing overly permissive policies through intelligent resource subsumption analysis.
//!
//! The merger implements the same logic as the TypeScript PolicyMerger, including:
//! - Resource relationship analysis (equivalent, subsumes, subsumed, incomparable)
//! - ARN prefix subsumption logic with regex pattern matching
//! - Statement grouping by mergeable resources to avoid over-permissive policies

use super::{Effect, IamPolicy, Statement};
use crate::{
    enrichment::Condition,
    errors::{ExtractorError, Result},
};
use regex::Regex;
use std::collections::HashSet;

/// IAM managed policy size limit in characters (excluding whitespace)
/// https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_iam-quotas.html
const IAM_MANAGED_POLICY_SIZE_LIMIT: usize = 6144;

/// Represents the relationship between two resources
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ResourceRelationship {
    /// Resources are identical
    Equivalent,
    /// First resource subsumes (is more general than) the second
    Subsumes,
    /// First resource is subsumed by (is more specific than) the second
    Subsumed,
    /// Resources are from different services or have no subsumption relationship
    Incomparable,
}

/// Groups statements with equivalent or incomparable resources that can be safely merged
#[derive(Debug, Clone)]
pub(crate) struct ResourceGroup {
    /// Set of all resources in this group
    pub(crate) resources: HashSet<String>,
    /// Set of all Conditions in this group
    pub(crate) conditions: HashSet<Condition>,
    /// Statements that belong to this group
    pub(crate) statements: Vec<Statement>,
}

impl ResourceGroup {
    /// Create a new resource group with the first statement
    pub(crate) fn new(statement: Statement) -> Self {
        let resources = statement.resource.iter().cloned().collect();
        let conditions = statement.condition.iter().cloned().collect();
        Self {
            resources,
            conditions,
            statements: vec![statement],
        }
    }

    /// Add a statement to this group and update the resource set
    pub(crate) fn add_statement(&mut self, statement: Statement) {
        self.resources.extend(statement.resource.iter().cloned());
        self.conditions.extend(statement.condition.iter().cloned());
        self.statements.push(statement);
    }
}

/// Configuration for policy merging behavior
#[derive(Debug, Clone, Default)]
pub struct PolicyMergerConfig {
    /// Allow merging actions from different services into the same statement
    pub(crate) allow_cross_service_merging: bool,
}

/// Policy merger for combining multiple IAM policies into a single optimized policy
#[derive(Debug, Clone)]
pub(crate) struct PolicyMerger {
    config: PolicyMergerConfig,
}

impl PolicyMerger {
    /// Create a new policy merger with default configuration
    pub(crate) fn new() -> Self {
        Self {
            config: PolicyMergerConfig::default(),
        }
    }

    /// Create a new policy merger with custom configuration
    pub(crate) fn with_config(config: PolicyMergerConfig) -> Self {
        Self { config }
    }

    /// Calculate the size of a policy in characters, excluding whitespace
    /// This matches AWS IAM's policy size calculation for managed policies
    fn calculate_policy_size(&self, policy: &IamPolicy) -> Result<usize> {
        let json = serde_json::to_string(policy).map_err(|e| {
            ExtractorError::policy_generation(format!(
                "Failed to serialize policy for size calculation: {e}"
            ))
        })?;

        // Count only non-whitespace characters
        let size = json.chars().filter(|c| !c.is_whitespace()).count();
        Ok(size)
    }

    /// Check if adding a statement to a policy would exceed the size limit
    fn would_exceed_size_limit(&self, policy: &IamPolicy, statement: &Statement) -> Result<bool> {
        let mut test_policy = policy.clone();
        test_policy.add_statement(statement.clone());
        let size = self.calculate_policy_size(&test_policy)?;
        Ok(size > IAM_MANAGED_POLICY_SIZE_LIMIT)
    }

    /// Merge multiple IAM policies into optimized policies with size limits
    ///
    /// This method combines statements from input policies using a size-aware approach.
    /// The size checking is handled at the statement grouping level to avoid creating
    /// merged statements that would exceed the policy size limit.
    ///
    /// # Arguments
    /// * `policies` - Slice of IAM policies to merge
    ///
    /// # Returns
    /// A vector of merged IAM policies, each staying within size limits
    ///
    /// # Errors
    /// Returns an error if regex compilation fails during ARN analysis
    pub(crate) fn merge_policies(&self, policies: &[IamPolicy]) -> Result<Vec<IamPolicy>> {
        if policies.is_empty() {
            return Ok(vec![]);
        }

        if policies.len() == 1 {
            // Single policy - still optimize its statements with size awareness
            return self.merge_statements(&policies[0].statements);
        }

        // Collect all statements from all policies
        let all_statements = self.collect_all_statements(policies);

        // Use size-aware merging
        self.merge_statements(&all_statements)
    }

    /// Merge statements with size awareness, creating multiple policies as needed
    ///
    /// This method groups statements by mergeable resources with size checking built-in,
    /// then distributes the resulting merged statements across multiple policies if needed.
    pub(crate) fn merge_statements(&self, statements: &[Statement]) -> Result<Vec<IamPolicy>> {
        if statements.is_empty() {
            return Ok(vec![IamPolicy::new()]);
        }

        // Group statements by mergeable resources with size awareness
        let groups = self.group_statements_by_mergeable_resources(statements)?;

        let mut policies = Vec::new();
        let mut current_policy = IamPolicy::new();

        // Process each group and add to policies with size checking
        for group in groups {
            let merged_statement = self.create_merged_statement_from_group(&group)?;

            // Check if adding this statement would exceed the size limit
            if self.would_exceed_size_limit(&current_policy, &merged_statement)? {
                // If current policy is not empty, save it and start a new one
                if !current_policy.statements.is_empty() {
                    policies.push(current_policy);
                    current_policy = IamPolicy::new();
                }

                // Add the statement to the new policy
                current_policy.add_statement(merged_statement);

                // Check if this single statement policy is still too large
                let single_statement_size = self.calculate_policy_size(&current_policy)?;
                if single_statement_size > IAM_MANAGED_POLICY_SIZE_LIMIT {
                    log::warn!(
                        "Single merged statement exceeds policy size limit ({single_statement_size} chars), including anyway"
                    );
                }
            } else {
                // Safe to add this statement to current policy
                current_policy.add_statement(merged_statement);
            }
        }

        // Add the last policy if it has statements
        if !current_policy.statements.is_empty() {
            policies.push(current_policy);
        }

        // Ensure we have at least one policy
        if policies.is_empty() {
            policies.push(IamPolicy::new());
        }

        if policies.len() > 1 {
            log::info!(
                "Created {} policies to stay within size limits",
                policies.len()
            );
        }

        Ok(policies)
    }

    /// Collect all statements from multiple policies
    fn collect_all_statements(&self, policies: &[IamPolicy]) -> Vec<Statement> {
        let mut statements = Vec::new();

        for policy in policies {
            statements.extend(policy.statements.clone());
        }

        statements
    }

    /// Group statements by mergeable resource sets with size awareness
    ///
    /// Statements can be merged if their resources are either:
    /// 1. Equivalent (identical)
    /// 2. Incomparable (no subsumption relationship)
    /// 3. The resulting merged statement would not exceed the policy size limit
    ///
    /// Statements with subsumption relationships are kept separate to avoid
    /// creating overly permissive policies. When size limits prevent merging
    /// with existing groups, new groups are started to allow future statements
    /// to potentially merge.
    fn group_statements_by_mergeable_resources(
        &self,
        statements: &[Statement],
    ) -> Result<Vec<ResourceGroup>> {
        // Only process Allow statements for merging
        let allow_statements: Vec<_> = statements
            .iter()
            .filter(|stmt| stmt.effect == Effect::Allow)
            .cloned()
            .collect();

        let mut groups: Vec<ResourceGroup> = Vec::new();

        for statement in allow_statements {
            let mut added_to_group = false;
            let mut size_prevented_merge = false;

            // Try to add this statement to an existing group
            for group in &mut groups {
                if self.can_merge_with_group(&statement, group)? {
                    // Check if merging would exceed size limits
                    if self.would_merged_statement_exceed_limit(group, &statement)? {
                        log::debug!("Size limit prevents merge with existing group");
                        size_prevented_merge = true;
                        continue; // Try next group
                    }

                    group.add_statement(statement.clone());
                    added_to_group = true;
                    break;
                }
            }

            // If not added to any existing group, create a new group
            // This allows future statements to potentially merge with this new group
            if !added_to_group {
                if size_prevented_merge {
                    log::debug!("Starting new group due to size constraints");
                }
                groups.push(ResourceGroup::new(statement));
            }
        }

        Ok(groups)
    }

    /// Check if merging a statement with a group would exceed the size limit
    fn would_merged_statement_exceed_limit(
        &self,
        group: &ResourceGroup,
        statement: &Statement,
    ) -> Result<bool> {
        // Create a temporary group with the new statement added
        let mut temp_group = group.clone();
        temp_group.add_statement(statement.clone());

        // Create the merged statement that would result from this group
        let merged_statement = self.create_merged_statement_from_group(&temp_group)?;

        // Create a temporary policy with just this statement to check size
        let mut temp_policy = IamPolicy::new();
        temp_policy.add_statement(merged_statement);

        let size = self.calculate_policy_size(&temp_policy)?;
        Ok(size > IAM_MANAGED_POLICY_SIZE_LIMIT)
    }

    /// Check if a statement can be merged with an existing group
    ///
    /// Returns true if resources are equivalent or incomparable.
    /// Returns false if any resource pair has a subsumption relationship.
    fn can_merge_with_group(&self, statement: &Statement, group: &ResourceGroup) -> Result<bool> {
        log::debug!("Trying to merge statement with group");
        log::debug!("  Statement conditions: {:?}", statement.condition);
        log::debug!("  Group conditions: {:?}", group.conditions);

        // Convert statement conditions to HashSet for comparison
        let stmt_conditions_set: std::collections::HashSet<_> =
            statement.condition.iter().collect();

        // Convert group conditions to HashSet for comparison
        let group_conditions_set: std::collections::HashSet<_> = group.conditions.iter().collect();

        // Conditions must be exactly the same to merge
        if stmt_conditions_set != group_conditions_set {
            log::debug!("Conditions differ, not merging:");
            log::debug!("  Statement condition set: {stmt_conditions_set:?}");
            log::debug!("  Group condition set: {group_conditions_set:?}");
            return Ok(false);
        }

        log::debug!("Conditions match, checking resources...");

        // Check for cross-service merging if not allowed
        if !self.config.allow_cross_service_merging {
            // Extract services from actions in the statement and group
            let stmt_services = self.extract_services_from_actions(&statement.action);
            let group_services = self.extract_services_from_group_actions(group);

            // Check if there are different services
            let all_services: std::collections::HashSet<_> =
                stmt_services.union(&group_services).collect();
            if all_services.len() > 1 {
                log::debug!(
                    "Cross-service merging not allowed, different services detected: {all_services:?}"
                );
                return Ok(false);
            }
        }

        // Check all pairwise combinations of resources
        for stmt_resource in &statement.resource {
            for group_resource in &group.resources {
                let relationship = self.get_resource_relationship(stmt_resource, group_resource)?;

                // If any pair has a subsumption relationship, cannot merge
                if relationship == ResourceRelationship::Subsumes
                    || relationship == ResourceRelationship::Subsumed
                {
                    log::debug!("Subsumes or Subsumed, not merging");
                    return Ok(false);
                }
            }
        }

        log::debug!("Merging");
        Ok(true)
    }

    /// Create a merged statement from a resource group
    fn create_merged_statement_from_group(&self, group: &ResourceGroup) -> Result<Statement> {
        let mut all_actions = HashSet::new();

        // Collect all actions from statements in this group
        for statement in &group.statements {
            for action in &statement.action {
                all_actions.insert(action.clone());
            }
        }

        // Sort actions and resources for consistent output
        let mut sorted_actions: Vec<_> = all_actions.into_iter().collect();
        sorted_actions.sort();

        let mut sorted_resources: Vec<_> = group.resources.iter().cloned().collect();
        sorted_resources.sort();

        let condition: Vec<_> = group.conditions.iter().cloned().collect();

        let result = Statement::allow(sorted_actions, sorted_resources).with_conditions(condition);

        log::debug!("From group:\n{group:?}\ncreated statement:\n{result:?}");

        Ok(result)
    }

    /// Determine the relationship between two resources
    ///
    /// Returns the relationship type based on resource analysis:
    /// - Equivalent: identical resources
    /// - Subsumes/Subsumed: one resource is more general/specific
    /// - Incomparable: different services or no subsumption relationship
    fn get_resource_relationship(
        &self,
        resource1: &str,
        resource2: &str,
    ) -> Result<ResourceRelationship> {
        // Exact match
        if resource1 == resource2 {
            return Ok(ResourceRelationship::Equivalent);
        }

        // Wildcard cases
        if resource1 == "*" {
            return Ok(if resource2 == "*" {
                ResourceRelationship::Equivalent
            } else {
                ResourceRelationship::Subsumes
            });
        }
        if resource2 == "*" {
            return Ok(ResourceRelationship::Subsumed);
        }

        // Both are ARNs - check for prefix subsumption
        if resource1.starts_with("arn:") && resource2.starts_with("arn:") {
            return self.get_arn_relationship(resource1, resource2);
        }

        // Different formats or services - incomparable
        Ok(ResourceRelationship::Incomparable)
    }

    /// Determine the relationship between two ARNs
    ///
    /// Analyzes ARN structure and resource patterns to determine subsumption relationships.
    /// Uses both prefix matching and regex pattern analysis.
    fn get_arn_relationship(&self, arn1: &str, arn2: &str) -> Result<ResourceRelationship> {
        // Parse ARN components: arn:partition:service:region:account:resource
        let parts1: Vec<&str> = arn1.split(':').collect();
        let parts2: Vec<&str> = arn2.split(':').collect();

        if parts1.len() < 6 || parts2.len() < 6 {
            return Ok(ResourceRelationship::Incomparable);
        }

        // Different services are incomparable
        if parts1[2] != parts2[2] {
            return Ok(ResourceRelationship::Incomparable);
        }

        // Same service - check for resource prefix subsumption
        let resource1 = parts1[5..].join(":");
        let resource2 = parts2[5..].join(":");

        // Check if these are different resource types within the same service
        // This prevents incorrect subsumption between different AWS resource types
        if self.are_different_resource_types(&resource1, &resource2) {
            return Ok(ResourceRelationship::Incomparable);
        }

        // Try regex-based subsumption logic first
        match self.check_regex_subsumption(&resource1, &resource2) {
            Ok(Some(relationship)) => return Ok(relationship),
            Ok(None) => {} // Continue with prefix logic
            Err(_) => {}   // Fall back to prefix logic on regex errors
        }

        // Check for prefix subsumption between wildcard resources (e.g., table/* vs table/MyTable/*)
        if resource1.ends_with("/*") && resource2.ends_with("/*") {
            let prefix1 = &resource1[..resource1.len() - 2];
            let prefix2 = &resource2[..resource2.len() - 2];

            if prefix1.starts_with(&format!("{prefix2}/")) || prefix1 == prefix2 {
                return Ok(ResourceRelationship::Subsumed); // arn1 is more specific
            }
            if prefix2.starts_with(&format!("{prefix1}/")) || prefix2 == prefix1 {
                return Ok(ResourceRelationship::Subsumes); // arn1 is more general
            }
        }

        // Same service but different specific resources - incomparable
        Ok(ResourceRelationship::Incomparable)
    }

    /// Check if two resource parts represent different resource types within the same service
    ///
    /// This helps prevent incorrect subsumption between fundamentally different resource types
    /// like S3 bucket objects vs S3 access points.
    fn are_different_resource_types(&self, resource1: &str, resource2: &str) -> bool {
        // Extract the resource type (first part before any slash or colon)
        let type1 = resource1
            .split('/')
            .next()
            .unwrap_or(resource1)
            .split(':')
            .next()
            .unwrap_or(resource1);
        let type2 = resource2
            .split('/')
            .next()
            .unwrap_or(resource2)
            .split(':')
            .next()
            .unwrap_or(resource2);

        // If the base resource types are different, they're incomparable
        // Examples:
        // - "bucket" vs "accesspoint" (S3)
        // - "table" vs "stream" (DynamoDB)
        // - "rule" vs "event-bus" (EventBridge)

        // Handle special cases for S3 where bucket objects have no explicit type prefix
        if (type1.is_empty() || type1 == "*") && !type2.is_empty() && type2 != "*" {
            return true; // bucket/* vs accesspoint/*
        }
        if (type2.is_empty() || type2 == "*") && !type1.is_empty() && type1 != "*" {
            return true; // accesspoint/* vs bucket/*
        }

        // Different non-empty types are incomparable
        if !type1.is_empty() && !type2.is_empty() && type1 != "*" && type2 != "*" && type1 != type2
        {
            return true;
        }

        false
    }

    /// Check for regex-based subsumption between two resource patterns
    ///
    /// Converts ARN patterns to regex and checks if one pattern subsumes another.
    /// Returns None if no subsumption relationship is found.
    fn check_regex_subsumption(
        &self,
        resource1: &str,
        resource2: &str,
    ) -> Result<Option<ResourceRelationship>> {
        // Convert ARN patterns to regex patterns
        let regex1 = self.arn_to_regex(resource1)?;
        let regex2 = self.arn_to_regex(resource2)?;

        // Check if resource1's pattern would match resource2 (resource1 subsumes resource2)
        if regex1.is_match(resource2) {
            log::debug!("check_regex_subsumption: {resource1} subsumes {resource2}");
            return Ok(Some(ResourceRelationship::Subsumes));
        }

        // Check if resource2's pattern would match resource1 (resource2 subsumes resource1)
        if regex2.is_match(resource1) {
            log::debug!("check_regex_subsumption: {resource2} subsumes {resource1}");
            return Ok(Some(ResourceRelationship::Subsumed));
        }

        // If neither pattern contains the other, no regex-based relationship
        Ok(None)
    }

    /// Convert an ARN pattern to a regular expression for subsumption checking
    ///
    /// Handles wildcards (*) and converts them to regex patterns for matching.
    fn arn_to_regex(&self, arn_pattern: &str) -> Result<Regex> {
        // Escape special regex characters except for our wildcards
        let mut regex_pattern = String::new();

        for ch in arn_pattern.chars() {
            match ch {
                '*' => regex_pattern.push_str(".*"), // Convert * to .* (match any characters)
                '.' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                    regex_pattern.push('\\');
                    regex_pattern.push(ch);
                }
                _ => regex_pattern.push(ch),
            }
        }

        // Anchor the pattern to match the entire string
        let anchored_pattern = format!("^{regex_pattern}$");

        Regex::new(&anchored_pattern).map_err(|e| {
            ExtractorError::policy_generation(format!(
                "Failed to compile regex for ARN pattern '{arn_pattern}': {e}"
            ))
        })
    }

    /// Remove subsumed resources from a list, keeping only the most general (subsuming) resources
    ///
    /// This function analyzes resource relationships and removes resources that are subsumed
    /// by other resources in the same list. For example, given:
    /// - "arn:aws:events:us-east-1:123456789012:rule/*"
    /// - "arn:aws:events:us-east-1:123456789012:rule/*/*"
    ///
    /// Only the first resource will be kept since it subsumes the second.
    ///
    /// # Arguments
    /// * `resources` - Vector of resource ARNs or patterns to deduplicate
    ///
    /// # Returns
    /// A new vector containing only the non-subsumed resources
    ///
    /// # Errors
    /// Returns an error if resource relationship analysis fails
    pub(crate) fn remove_subsumed_resources(&self, resources: Vec<String>) -> Result<Vec<String>> {
        if resources.len() <= 1 {
            return Ok(resources);
        }

        let mut non_subsumed = Vec::new();

        // For each resource, check if it's subsumed by any other resource
        for (i, resource) in resources.iter().enumerate() {
            let mut is_subsumed = false;

            // Compare with all other resources
            for (j, other_resource) in resources.iter().enumerate() {
                if i == j {
                    continue; // Skip self-comparison
                }

                // Check relationship between current resource and other resource
                let relationship = self.get_resource_relationship(resource, other_resource)?;

                // If current resource is subsumed by another, mark it for removal
                if relationship == ResourceRelationship::Subsumed {
                    is_subsumed = true;
                    break;
                }
            }

            // Keep resource if it's not subsumed by any other resource
            if !is_subsumed {
                non_subsumed.push(resource.clone());
            }
        }

        Ok(non_subsumed)
    }

    /// Extract service names from a list of actions
    fn extract_services_from_actions(
        &self,
        actions: &[String],
    ) -> std::collections::HashSet<String> {
        actions
            .iter()
            .filter_map(|action| {
                action
                    .split(':')
                    .next()
                    .map(std::string::ToString::to_string)
            })
            .collect()
    }

    /// Extract service names from all actions in a resource group
    fn extract_services_from_group_actions(
        &self,
        group: &ResourceGroup,
    ) -> std::collections::HashSet<String> {
        let mut services = std::collections::HashSet::new();
        for statement in &group.statements {
            let stmt_services = self.extract_services_from_actions(&statement.action);
            services.extend(stmt_services);
        }
        services
    }
}

impl Default for PolicyMerger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_statement(actions: Vec<&str>, resources: Vec<&str>) -> Statement {
        Statement::allow(
            actions.into_iter().map(String::from).collect(),
            resources.into_iter().map(String::from).collect(),
        )
    }

    #[test]
    fn test_resource_relationship_equivalent() {
        let merger = PolicyMerger::new();

        // Identical resources
        let rel = merger
            .get_resource_relationship("arn:aws:s3:::bucket/*", "arn:aws:s3:::bucket/*")
            .unwrap();
        assert_eq!(rel, ResourceRelationship::Equivalent);

        // Both wildcards
        let rel = merger.get_resource_relationship("*", "*").unwrap();
        assert_eq!(rel, ResourceRelationship::Equivalent);
    }

    #[test]
    fn test_resource_relationship_subsumption() {
        let merger = PolicyMerger::new();

        // Wildcard subsumes specific resource
        let rel = merger
            .get_resource_relationship("*", "arn:aws:s3:::bucket/*")
            .unwrap();
        assert_eq!(rel, ResourceRelationship::Subsumes);

        let rel = merger
            .get_resource_relationship("arn:aws:s3:::bucket/*", "*")
            .unwrap();
        assert_eq!(rel, ResourceRelationship::Subsumed);
    }

    #[test]
    fn test_arn_relationship_prefix_subsumption() {
        let merger = PolicyMerger::new();

        // Prefix subsumption
        let rel = merger
            .get_arn_relationship("arn:aws:s3:::bucket/*", "arn:aws:s3:::bucket/object.txt")
            .unwrap();
        assert_eq!(rel, ResourceRelationship::Subsumes);

        let rel = merger
            .get_arn_relationship("arn:aws:s3:::bucket/object.txt", "arn:aws:s3:::bucket/*")
            .unwrap();
        assert_eq!(rel, ResourceRelationship::Subsumed);
    }

    #[test]
    fn test_arn_relationship_different_services() {
        let merger = PolicyMerger::new();

        // Different services are incomparable
        let rel = merger
            .get_arn_relationship(
                "arn:aws:s3:::bucket/*",
                "arn:aws:dynamodb:us-east-1:123456789012:table/MyTable",
            )
            .unwrap();
        assert_eq!(rel, ResourceRelationship::Incomparable);
    }

    #[test]
    fn test_merge_equivalent_resources() {
        let merger = PolicyMerger::new();

        let statements = vec![
            create_test_statement(vec!["s3:GetObject"], vec!["arn:aws:s3:::bucket/*"]),
            create_test_statement(vec!["s3:PutObject"], vec!["arn:aws:s3:::bucket/*"]),
        ];

        let merged_policies = merger.merge_statements(&statements).unwrap();
        assert_eq!(merged_policies.len(), 1);
        assert_eq!(merged_policies[0].statements.len(), 1);

        let statement = &merged_policies[0].statements[0];
        assert_eq!(statement.action.len(), 2);
        assert!(statement.action.contains(&"s3:GetObject".to_string()));
        assert!(statement.action.contains(&"s3:PutObject".to_string()));
        assert_eq!(statement.resource, vec!["arn:aws:s3:::bucket/*"]);
    }

    #[test]
    fn test_no_merge_with_subsumption() {
        let merger = PolicyMerger::new();

        let statements = vec![
            create_test_statement(vec!["s3:GetObject"], vec!["arn:aws:s3:::bucket/*"]),
            create_test_statement(
                vec!["s3:PutObject"],
                vec!["arn:aws:s3:::bucket/specific.txt"],
            ),
        ];

        let merged_policies = merger.merge_statements(&statements).unwrap();
        // Should remain as separate statements due to subsumption
        assert_eq!(merged_policies.len(), 1);
        assert_eq!(merged_policies[0].statements.len(), 2);
    }

    #[test]
    fn test_merge_incomparable_resources() {
        let merger = PolicyMerger::new();

        // Test actions from the same service (S3) with incomparable resources
        // Bucket objects vs access points are different resource types within S3
        let statements = vec![
            create_test_statement(vec!["s3:GetObject"], vec!["arn:aws:s3:::bucket/*"]),
            create_test_statement(
                vec!["s3:GetAccessPoint"],
                vec!["arn:aws:s3:us-east-1:123456789012:accesspoint/my-access-point"],
            ),
        ];

        let merged_policies = merger.merge_statements(&statements).unwrap();
        // Should merge since same service but incomparable resource types
        assert_eq!(merged_policies.len(), 1);
        assert_eq!(merged_policies[0].statements.len(), 1);

        let statement = &merged_policies[0].statements[0];
        assert_eq!(statement.action.len(), 2);
        assert_eq!(statement.resource.len(), 2);
        assert!(statement.action.contains(&"s3:GetObject".to_string()));
        assert!(statement.action.contains(&"s3:GetAccessPoint".to_string()));
    }

    #[test]
    fn test_no_merge_different_services_when_cross_service_disabled() {
        let merger = PolicyMerger::new(); // Default config has cross-service merging disabled

        let statements = vec![
            create_test_statement(vec!["s3:GetObject"], vec!["arn:aws:s3:::bucket/*"]),
            create_test_statement(
                vec!["dynamodb:GetItem"],
                vec!["arn:aws:dynamodb:us-east-1:123456789012:table/MyTable"],
            ),
        ];

        let merged_policies = merger.merge_statements(&statements).unwrap();
        // Should NOT merge since different services and cross-service merging is disabled by default
        assert_eq!(merged_policies.len(), 1);
        assert_eq!(merged_policies[0].statements.len(), 2);

        // Verify each statement remains separate
        let s3_statement = merged_policies[0]
            .statements
            .iter()
            .find(|stmt| stmt.action.contains(&"s3:GetObject".to_string()))
            .expect("Should find S3 statement");
        assert_eq!(s3_statement.action.len(), 1);
        assert_eq!(s3_statement.resource.len(), 1);

        let dynamodb_statement = merged_policies[0]
            .statements
            .iter()
            .find(|stmt| stmt.action.contains(&"dynamodb:GetItem".to_string()))
            .expect("Should find DynamoDB statement");
        assert_eq!(dynamodb_statement.action.len(), 1);
        assert_eq!(dynamodb_statement.resource.len(), 1);
    }

    #[test]
    fn test_merge_multiple_policies() {
        let merger = PolicyMerger::new();

        let mut policy1 = IamPolicy::new();
        policy1.add_statement(create_test_statement(
            vec!["s3:GetObject"],
            vec!["arn:aws:s3:::bucket1/*"],
        ));

        let mut policy2 = IamPolicy::new();
        policy2.add_statement(create_test_statement(
            vec!["s3:PutObject"],
            vec!["arn:aws:s3:::bucket1/*"],
        ));

        let merged_policies = merger.merge_policies(&[policy1, policy2]).unwrap();
        assert_eq!(merged_policies.len(), 1);
        assert_eq!(merged_policies[0].statements.len(), 1);

        let statement = &merged_policies[0].statements[0];
        assert_eq!(statement.action.len(), 2);
        assert!(statement.action.contains(&"s3:GetObject".to_string()));
        assert!(statement.action.contains(&"s3:PutObject".to_string()));
    }

    #[test]
    fn test_empty_policies() {
        let merger = PolicyMerger::new();

        let merged_policies = merger.merge_policies(&[]).unwrap();
        assert!(merged_policies.is_empty());

        let empty_policy = IamPolicy::new();
        let merged_policies = merger.merge_policies(&[empty_policy]).unwrap();
        assert_eq!(merged_policies.len(), 1);
        assert_eq!(merged_policies[0].statements.len(), 0);
    }

    #[test]
    fn test_arn_to_regex() {
        let merger = PolicyMerger::new();

        let regex = merger.arn_to_regex("bucket/*").unwrap();
        assert!(regex.is_match("bucket/object.txt"));
        assert!(regex.is_match("bucket/folder/object.txt"));
        assert!(!regex.is_match("other-bucket/object.txt"));

        let regex = merger.arn_to_regex("table/MyTable").unwrap();
        assert!(regex.is_match("table/MyTable"));
        assert!(!regex.is_match("table/MyTable/item"));
    }

    #[test]
    fn test_remove_subsumed_resources_basic_subsumption() {
        let merger = PolicyMerger::new();

        // Test the example case from the requirements
        let resources = vec![
            "arn:aws:events:us-east-1:123456789012:rule/*".to_string(),
            "arn:aws:events:us-east-1:123456789012:rule/*/*".to_string(),
        ];

        let result = merger.remove_subsumed_resources(resources).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "arn:aws:events:us-east-1:123456789012:rule/*");
    }

    #[test]
    fn test_remove_subsumed_resources_reverse_order() {
        let merger = PolicyMerger::new();

        // Test with subsumed resource first
        let resources = vec![
            "arn:aws:events:us-east-1:123456789012:rule/*/*".to_string(),
            "arn:aws:events:us-east-1:123456789012:rule/*".to_string(),
        ];

        let result = merger.remove_subsumed_resources(resources).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "arn:aws:events:us-east-1:123456789012:rule/*");
    }

    #[test]
    fn test_remove_subsumed_resources_wildcard_subsumption() {
        let merger = PolicyMerger::new();

        let resources = vec![
            "*".to_string(),
            "arn:aws:s3:::bucket/*".to_string(),
            "arn:aws:dynamodb:us-east-1:123456789012:table/MyTable".to_string(),
        ];

        let result = merger.remove_subsumed_resources(resources).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "*");
    }

    #[test]
    fn test_remove_subsumed_resources_no_subsumption() {
        let merger = PolicyMerger::new();

        // Different services - should be incomparable, no subsumption
        let resources = vec![
            "arn:aws:s3:::bucket/*".to_string(),
            "arn:aws:dynamodb:us-east-1:123456789012:table/MyTable".to_string(),
        ];

        let result = merger.remove_subsumed_resources(resources).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"arn:aws:s3:::bucket/*".to_string()));
        assert!(
            result.contains(&"arn:aws:dynamodb:us-east-1:123456789012:table/MyTable".to_string())
        );
    }

    #[test]
    fn test_remove_subsumed_resources_equivalent_resources() {
        let merger = PolicyMerger::new();

        // Identical resources should be kept (only one copy)
        let resources = vec![
            "arn:aws:s3:::bucket/*".to_string(),
            "arn:aws:s3:::bucket/*".to_string(),
        ];

        let result = merger.remove_subsumed_resources(resources).unwrap();
        assert_eq!(result.len(), 2); // Both kept since they're equivalent, not subsumed
    }

    #[test]
    fn test_remove_subsumed_resources_complex_hierarchy() {
        let merger = PolicyMerger::new();

        let resources = vec![
            "arn:aws:s3:::bucket/folder1/*".to_string(),
            "arn:aws:s3:::bucket/*".to_string(),
            "arn:aws:s3:::bucket/folder1/subfolder/*".to_string(),
            "arn:aws:s3:::bucket/folder2/*".to_string(),
        ];

        let result = merger.remove_subsumed_resources(resources).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "arn:aws:s3:::bucket/*");
    }

    #[test]
    fn test_remove_subsumed_resources_empty_and_single() {
        let merger = PolicyMerger::new();

        // Empty vector
        let result = merger.remove_subsumed_resources(vec![]).unwrap();
        assert_eq!(result.len(), 0);

        // Single resource
        let resources = vec!["arn:aws:s3:::bucket/*".to_string()];
        let result = merger.remove_subsumed_resources(resources).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "arn:aws:s3:::bucket/*");
    }

    #[test]
    fn test_remove_subsumed_resources_preserves_order() {
        let merger = PolicyMerger::new();

        // Test that order is preserved for non-subsumed resources
        let resources = vec![
            "arn:aws:s3:::bucket-a/*".to_string(),
            "arn:aws:dynamodb:us-east-1:123456789012:table/TableA".to_string(),
            "arn:aws:s3:::bucket-b/*".to_string(),
            "arn:aws:dynamodb:us-east-1:123456789012:table/TableB".to_string(),
        ];

        let result = merger.remove_subsumed_resources(resources.clone()).unwrap();
        assert_eq!(result.len(), 4);
        assert_eq!(result, resources); // Order should be preserved
    }

    #[test]
    fn test_s3_bucket_vs_accesspoint_relationship() {
        let merger = PolicyMerger::new();

        // Test the specific case that ensures different S3 resource types are incomparable
        let bucket_resource = "arn:aws:s3:::*/*";
        let accesspoint_resource = "arn:aws:s3:us-east-1:123456789012:accesspoint/*";

        let relationship = merger
            .get_resource_relationship(bucket_resource, accesspoint_resource)
            .unwrap();

        // These should be incomparable since they're different resource types within S3
        assert_eq!(relationship, ResourceRelationship::Incomparable);

        // Test the reverse relationship too
        let reverse_relationship = merger
            .get_resource_relationship(accesspoint_resource, bucket_resource)
            .unwrap();
        assert_eq!(reverse_relationship, ResourceRelationship::Incomparable);
    }

    #[test]
    fn test_calculate_policy_size() {
        let merger = PolicyMerger::new();

        let mut policy = IamPolicy::new();
        policy.add_statement(create_test_statement(
            vec!["s3:GetObject"],
            vec!["arn:aws:s3:::bucket/*"],
        ));

        let size = merger.calculate_policy_size(&policy).unwrap();

        // Size should be greater than 0 and should not count whitespace
        assert!(size > 0);

        // Verify that whitespace is not counted by comparing with manual calculation
        let json = serde_json::to_string(&policy).unwrap();
        let expected_size = json.chars().filter(|c| !c.is_whitespace()).count();
        assert_eq!(size, expected_size);
    }

    #[test]
    fn test_would_exceed_size_limit() {
        let merger = PolicyMerger::new();

        let mut policy = IamPolicy::new();
        let small_statement =
            create_test_statement(vec!["s3:GetObject"], vec!["arn:aws:s3:::bucket/*"]);

        // Adding a small statement to empty policy should not exceed limit
        assert!(!merger
            .would_exceed_size_limit(&policy, &small_statement)
            .unwrap());

        // Add the statement
        policy.add_statement(small_statement.clone());

        // Adding another small statement should still not exceed limit
        assert!(!merger
            .would_exceed_size_limit(&policy, &small_statement)
            .unwrap());
    }

    #[test]
    fn test_size_aware_grouping() {
        let merger = PolicyMerger::new();

        // Create statements that would be mergeable but might exceed size limits
        let statements = vec![
            create_test_statement(vec!["s3:GetObject"], vec!["arn:aws:s3:::bucket/*"]),
            create_test_statement(vec!["s3:PutObject"], vec!["arn:aws:s3:::bucket/*"]),
        ];

        let groups = merger
            .group_statements_by_mergeable_resources(&statements)
            .unwrap();

        // These should be grouped together since they're small and mergeable
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].statements.len(), 2);
    }

    #[test]
    fn test_merge_policies_with_size_limits() {
        let merger = PolicyMerger::new();

        // Create policies with statements that should be merged
        let mut policy1 = IamPolicy::new();
        policy1.add_statement(create_test_statement(
            vec!["s3:GetObject"],
            vec!["arn:aws:s3:::bucket1/*"],
        ));

        let mut policy2 = IamPolicy::new();
        policy2.add_statement(create_test_statement(
            vec!["s3:PutObject"],
            vec!["arn:aws:s3:::bucket1/*"],
        ));

        let merged_policies = merger.merge_policies(&[policy1, policy2]).unwrap();

        // Should result in at least one policy
        assert!(!merged_policies.is_empty());

        // Verify all policies are within size limits
        for policy in &merged_policies {
            let size = merger.calculate_policy_size(policy).unwrap();
            if policy.statements.len() > 1 {
                // Allow single large statements
                assert!(
                    size <= IAM_MANAGED_POLICY_SIZE_LIMIT,
                    "Policy size {} exceeds limit {}",
                    size,
                    IAM_MANAGED_POLICY_SIZE_LIMIT
                );
            }
        }
    }

    #[test]
    fn test_would_merged_statement_exceed_limit() {
        let merger = PolicyMerger::new();

        // Create a small group and statement
        let statement1 = create_test_statement(vec!["s3:GetObject"], vec!["arn:aws:s3:::bucket/*"]);
        let statement2 = create_test_statement(vec!["s3:PutObject"], vec!["arn:aws:s3:::bucket/*"]);

        let group = ResourceGroup::new(statement1);

        // These small statements should not exceed the limit when merged
        assert!(!merger
            .would_merged_statement_exceed_limit(&group, &statement2)
            .unwrap());
    }

    #[test]
    fn test_policy_exceeding_limit_forces_split() {
        let merger = PolicyMerger::new();

        // Create a policy with many statements that would exceed the 6144 character limit
        // Use different services to prevent merging
        let mut statements = Vec::new();

        for i in 0..30 {
            let long_resource = format!(
                "arn:aws:s3:::very-long-bucket-name-with-many-characters-to-make-policy-large-{}/path/to/very/long/object/name/that/contributes/to/size/unique-object-{}/*",
                "x".repeat(100), i
            );

            statements.push(create_test_statement(
                vec!["s3:GetObject"],
                vec![&long_resource],
            ));
        }

        // Add some DynamoDB statements to prevent cross-service merging
        for i in 0..20 {
            let long_resource = format!(
                "arn:aws:dynamodb:us-east-1:123456789012:table/VeryLongTableNameWithManyCharactersToMakePolicyLarge-{}/index/VeryLongIndexName-{}",
                "x".repeat(100), i
            );

            statements.push(create_test_statement(
                vec!["dynamodb:GetItem"],
                vec![&long_resource],
            ));
        }

        let large_policy = IamPolicy {
            id: "IamPolicyAutoPilot".to_string(),
            version: "2012-10-17".to_string(),
            statements,
        };

        // Verify the policy would exceed the limit
        let policy_size = merger.calculate_policy_size(&large_policy).unwrap();
        assert!(
            policy_size > IAM_MANAGED_POLICY_SIZE_LIMIT,
            "Test policy size {} should exceed limit {} for this test to be valid",
            policy_size,
            IAM_MANAGED_POLICY_SIZE_LIMIT
        );

        // Merge should split this into multiple policies
        let result = merger.merge_policies(&[large_policy]).unwrap();

        // Should result in multiple policies
        assert!(
            result.len() > 1,
            "Large policy should be split into multiple policies, got {}",
            result.len()
        );

        // Each resulting policy should be under the limit (except for single oversized statements)
        for (i, policy) in result.iter().enumerate() {
            let size = merger.calculate_policy_size(policy).unwrap();
            if policy.statements.len() > 1 {
                assert!(
                    size <= IAM_MANAGED_POLICY_SIZE_LIMIT,
                    "Split policy {} has size {} which exceeds limit {}",
                    i,
                    size,
                    IAM_MANAGED_POLICY_SIZE_LIMIT
                );
            }
            assert!(
                !policy.statements.is_empty(),
                "Split policy {} should not be empty",
                i
            );
        }

        // Verify that we have at least some statements (they may be merged but should be preserved)
        let total_statements: usize = result.iter().map(|p| p.statements.len()).sum();
        assert!(
            total_statements >= 2,
            "Should have at least 2 statements after merging (S3 and DynamoDB), got {}",
            total_statements
        );
        assert!(
            total_statements <= 50,
            "Should not have more statements than we started with, got {}",
            total_statements
        );
    }

    #[test]
    fn test_multiple_large_policies_distribution() {
        let merger = PolicyMerger::new();

        // Create multiple policies that individually are large but under the limit
        let mut policies = Vec::new();

        for policy_idx in 0..5 {
            let mut statements = Vec::new();

            // Each policy has statements that make it close to but under the limit
            for stmt_idx in 0..15 {
                let resource = format!(
                    "arn:aws:dynamodb:us-east-1:123456789012:table/LargeTableName{}-{}/index/LargeIndexName-{}",
                    policy_idx, stmt_idx, "x".repeat(30)
                );

                let mut statement = create_test_statement(
                    vec![
                        "dynamodb:GetItem",
                        "dynamodb:PutItem",
                        "dynamodb:UpdateItem",
                        "dynamodb:DeleteItem",
                        "dynamodb:Query",
                        "dynamodb:Scan",
                    ],
                    vec![&resource],
                );

                // Add conditions to increase size
                use crate::enrichment::{Condition, Operator};
                let condition = Condition {
                    operator: Operator::StringEquals,
                    key: "dynamodb:LeadingKeys".to_string(),
                    values: vec![format!("user-{}", stmt_idx)],
                };
                statement.condition.push(condition);

                statements.push(statement);
            }

            policies.push(IamPolicy {
                id: "IamPolicyAutoPilot".to_string(),
                version: "2012-10-17".to_string(),
                statements,
            });
        }

        // Verify each individual policy is under the limit but merging all would exceed it
        for (i, policy) in policies.iter().enumerate() {
            let size = merger.calculate_policy_size(policy).unwrap();
            assert!(
                size <= IAM_MANAGED_POLICY_SIZE_LIMIT,
                "Individual policy {} size {} should be under limit {}",
                i,
                size,
                IAM_MANAGED_POLICY_SIZE_LIMIT
            );
        }

        // Create a single merged policy to verify it would exceed the limit
        let all_statements: Vec<Statement> =
            policies.iter().flat_map(|p| p.statements.clone()).collect();
        let hypothetical_merged = IamPolicy {
            id: "IamPolicyAutoPilot".to_string(),
            version: "2012-10-17".to_string(),
            statements: all_statements,
        };
        let merged_size = merger.calculate_policy_size(&hypothetical_merged).unwrap();
        assert!(
            merged_size > IAM_MANAGED_POLICY_SIZE_LIMIT,
            "Merged policy size {} should exceed limit {} for this test to be valid",
            merged_size,
            IAM_MANAGED_POLICY_SIZE_LIMIT
        );

        // Now test the actual merge function
        let result = merger.merge_policies(&policies).unwrap();

        // Should result in multiple policies due to size constraints
        assert!(
            result.len() > 1,
            "Should split into multiple policies due to size constraints"
        );

        // Each resulting policy should be under the limit (except for single oversized statements)
        for (i, policy) in result.iter().enumerate() {
            let size = merger.calculate_policy_size(policy).unwrap();
            if policy.statements.len() > 1 {
                assert!(
                    size <= IAM_MANAGED_POLICY_SIZE_LIMIT,
                    "Result policy {} has size {} which exceeds limit {}",
                    i,
                    size,
                    IAM_MANAGED_POLICY_SIZE_LIMIT
                );
            }
        }

        // Verify that statements are reasonably preserved (they may be merged)
        let total_statements: usize = result.iter().map(|p| p.statements.len()).sum();
        let original_statements: usize = policies.iter().map(|p| p.statements.len()).sum();
        assert!(
            total_statements >= 5,
            "Should have at least 5 statements after merging, got {}",
            total_statements
        );
        assert!(
            total_statements <= original_statements,
            "Should not have more statements than we started with, got {} vs {}",
            total_statements,
            original_statements
        );
    }

    #[test]
    fn test_single_statement_exceeding_limit() {
        let merger = PolicyMerger::new();

        // Create a single statement that by itself would exceed the limit
        // This tests the edge case where even a single statement is too large
        let enormous_resource = format!(
            "arn:aws:s3:::bucket-with-extremely-long-name-{}/path/to/object/with/very/long/path/{}/*",
            "x".repeat(1000), "y".repeat(5000)
        );

        let large_statement = create_test_statement(vec!["s3:GetObject"], vec![&enormous_resource]);

        let policy = IamPolicy {
            id: "IamPolicyAutoPilot".to_string(),
            version: "2012-10-17".to_string(),
            statements: vec![large_statement],
        };

        // Verify this single policy exceeds the limit
        let policy_size = merger.calculate_policy_size(&policy).unwrap();
        assert!(
            policy_size > IAM_MANAGED_POLICY_SIZE_LIMIT,
            "Single statement policy size {} should exceed limit {} for this test",
            policy_size,
            IAM_MANAGED_POLICY_SIZE_LIMIT
        );

        // The merge function should still handle this gracefully
        // Even if a single statement exceeds the limit, it should be preserved
        let result = merger.merge_policies(&[policy]).unwrap();

        assert_eq!(
            result.len(),
            1,
            "Should return exactly one policy even if it exceeds the limit"
        );
        assert_eq!(
            result[0].statements.len(),
            1,
            "Should preserve the single statement"
        );

        // The statement should be preserved even though it exceeds the limit
        // This is because we can't split a single statement
        let result_size = merger.calculate_policy_size(&result[0]).unwrap();
        assert_eq!(result_size, policy_size, "Policy size should be preserved");
    }

    #[test]
    fn test_large_statement_grouping_with_size_limits() {
        let merger = PolicyMerger::new();

        // Create statements with very long resources that would create large merged statements
        let mut statements = Vec::new();

        for i in 0..20 {
            let long_resource = format!(
                "arn:aws:lambda:us-east-1:123456789012:function:VeryLongFunctionNameThatMakesThePolicyLarge-{}-{}",
                "x".repeat(100), i
            );

            statements.push(create_test_statement(
                vec![
                    "lambda:InvokeFunction",
                    "lambda:GetFunction",
                    "lambda:UpdateFunctionCode",
                ],
                vec![&long_resource],
            ));
        }

        // Test the grouping function directly
        let groups = merger
            .group_statements_by_mergeable_resources(&statements)
            .unwrap();

        // Should create at least one group
        assert!(
            !groups.is_empty(),
            "Should create at least one group, got {}",
            groups.len()
        );

        // Verify each group would create a statement under the limit
        for (i, group) in groups.iter().enumerate() {
            let merged_statement = merger.create_merged_statement_from_group(group).unwrap();
            let mut temp_policy = IamPolicy::new();
            temp_policy.add_statement(merged_statement);
            let size = merger.calculate_policy_size(&temp_policy).unwrap();

            if group.statements.len() > 1 {
                assert!(
                    size <= IAM_MANAGED_POLICY_SIZE_LIMIT,
                    "Group {} merged statement size {} should not exceed limit {}",
                    i,
                    size,
                    IAM_MANAGED_POLICY_SIZE_LIMIT
                );
            }
        }

        // Verify all statements are preserved across groups
        let total_statements: usize = groups.iter().map(|g| g.statements.len()).sum();
        assert_eq!(
            total_statements,
            statements.len(),
            "All statements should be preserved across groups"
        );
    }

    #[test]
    fn test_catalog_wildcard_does_not_subsume_catalog() {
        let merger = PolicyMerger::new();

        // Test the specific case where catalog/* should NOT subsume catalog
        // This is important for AWS Glue resources where these represent different resource types
        let catalog_wildcard = "catalog/*";
        let catalog_base = "catalog";

        let relationship = merger
            .get_resource_relationship(catalog_wildcard, catalog_base)
            .unwrap();

        // catalog/* should NOT subsume catalog - they should be incomparable
        // because they represent different resource patterns within the same service
        assert_eq!(
            relationship,
            ResourceRelationship::Incomparable,
            "catalog/* should not subsume catalog - they should be incomparable"
        );

        // Test the reverse relationship too
        let reverse_relationship = merger
            .get_resource_relationship(catalog_base, catalog_wildcard)
            .unwrap();
        assert_eq!(
            reverse_relationship,
            ResourceRelationship::Incomparable,
            "catalog should not be subsumed by catalog/* - they should be incomparable"
        );
    }

    #[test]
    fn test_catalog_wildcard_arn_does_not_subsume_catalog_arn() {
        let merger = PolicyMerger::new();

        // Test with full ARNs for AWS Glue catalog resources
        let catalog_wildcard_arn = "arn:aws:glue:us-east-1:123456789012:catalog/*";
        let catalog_base_arn = "arn:aws:glue:us-east-1:123456789012:catalog";

        let relationship = merger
            .get_arn_relationship(catalog_wildcard_arn, catalog_base_arn)
            .unwrap();

        // These should be incomparable, not subsumption
        assert_eq!(
            relationship,
            ResourceRelationship::Incomparable,
            "arn:aws:glue:*:*:catalog/* should not subsume arn:aws:glue:*:*:catalog"
        );

        // Test the reverse relationship too
        let reverse_relationship = merger
            .get_arn_relationship(catalog_base_arn, catalog_wildcard_arn)
            .unwrap();
        assert_eq!(
            reverse_relationship,
            ResourceRelationship::Incomparable,
            "arn:aws:glue:*:*:catalog should not be subsumed by arn:aws:glue:*:*:catalog/*"
        );
    }
}
