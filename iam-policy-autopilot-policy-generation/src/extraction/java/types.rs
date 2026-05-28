//! Java-specific intermediate types for the extraction phase.
//!
//! These types are produced by the Java extractors and consumed by [`JavaMatcher`].
//! They are **not** exposed outside the `extraction::java` module.
//!
//! [`JavaMatcher`]: super::matcher::JavaMatcher
use crate::extraction::Parameter;
use crate::Location;

// ================================================================================================
// Import
// ================================================================================================

/// An import statement extracted from a Java source file.
///
/// Used by [`JavaMatcher`] to narrow the set of candidate AWS services for a given
/// method call (import-based filtering).
///
/// [`JavaMatcher`]: super::matcher::JavaMatcher
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Import {
    /// Raw import path, e.g. `"software.amazon.awssdk.services.s3.S3Client"`
    pub(crate) expr: String,
    /// Extracted AWS service name, e.g. `"s3"` (from the `services.<name>` segment).
    /// Only imports that resolve to an AWS service are stored; non-AWS imports are discarded.
    pub(crate) service: String,
    /// Source location of the import declaration
    pub(crate) location: Location,
    /// `true` for `import static ...` declarations
    pub(crate) is_static: bool,
}

// ================================================================================================
// Call
// ================================================================================================

/// A method call extracted from a Java source file.
///
/// Represents `receiver.method(args)` patterns. The receiver and parameters are used by
/// [`JavaMatcher`] for import-based and parameter-based filtering.
///
/// [`JavaMatcher`]: super::matcher::JavaMatcher
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Call {
    /// Raw expression, e.g. `"client.putObject(request)"`
    pub(crate) expr: String,
    /// Method name, e.g. `"putObject"`
    pub(crate) method: String,
    /// Parsed arguments (reuses the existing [`Parameter`] type)
    pub(crate) parameters: Vec<Parameter>,
    /// Source location of the method invocation.
    pub(crate) location: Location,
    /// The receiver variable's declaration, if found by the scope walk.
    /// `None` when the receiver is a field access / getter (Tier 2/3) or when the
    /// declaration is not reachable from the call site.
    #[serde(default)]
    pub(crate) receiver_declaration: Option<ReceiverDeclaration>,
}

// ================================================================================================
// ReceiverDeclaration
// ================================================================================================

/// Information about the receiver variable's declaration site for a method call.
///
/// Captured inline by the extractor during the scope walk when the receiver variable's
/// declaration is found in the AST. `None` when the receiver is a field access, getter,
/// or when the declaration is not reachable from the call site.
///
/// For local variable declarations, `expr` is the initializer expression
/// (e.g. `"s3Client.waiter()"` or `"S3Client.create()"`).
/// For formal parameters, `expr` is the full parameter declaration text
/// (e.g. `"S3Waiter waiter"` or `"S3Client s3"`).
///
/// Used by [`Waiter`], [`Paginator`], and [`Call`] to carry the resolved
/// receiver type to the matcher.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ReceiverDeclaration {
    /// The declaration expression:
    /// - For local vars: the initializer, e.g. `"s3Client.waiter()"` or `"S3Client.create()"`
    /// - For formal params: the full parameter declaration, e.g. `"S3Waiter waiter"` or `"S3Client s3"`
    pub(crate) expr: String,
    /// Declared type of the receiver variable, e.g. `"S3Waiter"` or `"S3Client"`.
    /// `None` when declared with `var` (inferred type).
    pub(crate) type_name: Option<String>,
    /// Source location of the declaration
    pub(crate) location: Location,
}

// ================================================================================================
// Waiter
// ================================================================================================

/// A `waitUntil*` call extracted from a Java source file.
///
/// Represents a single `receiver.waitUntilFoo(args)` invocation. When the receiver variable's
/// declaration is reachable from the call site via a scope walk, information about the receiver's
/// declaration is stored in `receiver_declaration`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Waiter {
    /// Raw expression of the `waitUntil*` call
    pub(crate) expr: String,
    /// The wait method name with the `"waitUntil"` prefix stripped and converted to camelCase,
    /// e.g. `"bucketExists"` for `waitUntilBucketExists`
    pub(crate) name: String,
    /// Positional arguments passed to the `waitUntil*` call
    pub(crate) parameters: Vec<Parameter>,
    /// Source location of the `waitUntil*` call
    pub(crate) location: Location,
    /// The receiver variable's declaration, if found by the scope walk.
    /// `None` when the receiver is a field access / getter (Tier 2/3) or when the
    /// declaration is not reachable from the call site.
    #[serde(default)]
    pub(crate) receiver_declaration: Option<ReceiverDeclaration>,
}

// ================================================================================================
// Paginator
// ================================================================================================

/// A paginator usage extracted from a Java source file.
///
/// Example: `s3Client.listObjectsV2Paginator(request)` → `operation = "listObjectsV2"`
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Paginator {
    /// Raw expression
    pub(crate) expr: String,
    /// Base operation name with `"Paginator"` suffix stripped, e.g. `"listObjectsV2"`
    pub(crate) operation: String,
    /// Positional arguments passed to the paginator call
    pub(crate) parameters: Vec<Parameter>,
    /// Source location
    pub(crate) location: Location,
    /// The receiver variable's declaration, if found by the scope walk.
    /// `None` when the receiver is a field access / getter (Tier 2/3) or when the
    /// declaration is not reachable from the call site.
    #[serde(default)]
    pub(crate) receiver_declaration: Option<ReceiverDeclaration>,
}

// ================================================================================================
// UtilityImport
// ================================================================================================

/// A utility import extracted from a Java source file.
///
/// Covers non-`services.*` AWS SDK packages such as `transfer.s3`, `enhanced.dynamodb`,
/// `auth.credentials`, and `core.async`. Used by [`UtilityMatcher`] to resolve
/// receiver variable names to utility class names.
///
/// [`UtilityMatcher`]: super::matchers::utility
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct UtilityImport {
    /// Raw import path, e.g. `"software.amazon.awssdk.transfer.s3.S3TransferManager"`
    pub(crate) expr: String,
    /// Logical utility name, e.g. `"s3-transfer"` (from the mapping table)
    pub(crate) utility_name: String,
    /// Simple class name (last segment), e.g. `"S3TransferManager"`
    pub(crate) class_name: String,
    /// Source location of the import declaration
    pub(crate) location: Location,
}

// ================================================================================================
// ExtractionResult
// ================================================================================================

/// All data extracted from a single Java source file by the [`JavaLanguageExtractorSet`].
///
/// This is the intermediate representation consumed by [`JavaMatcher`] to produce
/// the final [`Vec<SdkMethodCall>`].
///
/// [`JavaLanguageExtractorSet`]: super::extractor::JavaLanguageExtractorSet
/// [`JavaMatcher`]: super::matcher::JavaMatcher
/// [`Vec<SdkMethodCall>`]: crate::SdkMethodCall
#[derive(Default, Debug)]
pub(crate) struct ExtractionResult {
    /// Import declarations found in the file
    pub(crate) imports: Vec<Import>,
    /// Utility (non-`services.*`) import declarations found in the file
    pub(crate) utility_imports: Vec<UtilityImport>,
    /// Direct method calls (`receiver.method(args)`)
    pub(crate) calls: Vec<Call>,
    /// Waiter usages
    pub(crate) waiters: Vec<Waiter>,
    /// Paginator usages
    pub(crate) paginators: Vec<Paginator>,
}

impl ExtractionResult {
    /// Merge another result into this one (used when combining per-extractor outputs).
    pub(crate) fn extend(&mut self, other: Self) {
        self.imports.extend(other.imports);
        self.utility_imports.extend(other.utility_imports);
        self.calls.extend(other.calls);
        self.waiters.extend(other.waiters);
        self.paginators.extend(other.paginators);
    }
}
