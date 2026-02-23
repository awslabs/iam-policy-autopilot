//! Result type alias for operations that can fail with `ExtractorError`
use ast_grep_language::{Go, JavaScript, Python, TypeScript};
use async_trait::async_trait;

use crate::extraction::go::types::GoImportInfo;
use crate::extraction::AstWithSourceFile;
use crate::{SdkMethodCall, ServiceModelIndex, SourceFile};

/// Enum to handle different AST types from different languages
#[derive(Clone)]
pub(crate) enum ExtractorResult {
    Python(AstWithSourceFile<Python>, Vec<SdkMethodCall>),
    Go(AstWithSourceFile<Go>, Vec<SdkMethodCall>, GoImportInfo),
    JavaScript(AstWithSourceFile<JavaScript>, Vec<SdkMethodCall>),
    TypeScript(AstWithSourceFile<TypeScript>, Vec<SdkMethodCall>),
}

impl ExtractorResult {
    /// Extract just the method calls from the result
    pub(crate) fn method_calls(self) -> Vec<SdkMethodCall> {
        match self {
            Self::Python(_, calls) => calls,
            Self::Go(_, calls, _) => calls,
            Self::JavaScript(_, calls) => calls,
            Self::TypeScript(_, calls) => calls,
        }
    }

    /// Get a reference to the method calls without consuming the result
    #[allow(dead_code)]
    pub(crate) fn method_calls_ref(&self) -> &Vec<SdkMethodCall> {
        match self {
            Self::Python(_, calls) => calls,
            Self::Go(_, calls, _) => calls,
            Self::JavaScript(_, calls) => calls,
            Self::TypeScript(_, calls) => calls,
        }
    }

    /// Get a reference to the import information for Go results
    #[allow(dead_code)]
    pub(crate) fn go_import_info(&self) -> Option<&GoImportInfo> {
        match self {
            Self::Go(_, _, import_info) => Some(import_info),
            _ => None,
        }
    }
}

/// Extractor trait
#[async_trait]
pub(crate) trait Extractor: Send + Sync {
    /// Parse source code into method calls and return the AST
    async fn parse(&self, source_file: &SourceFile) -> ExtractorResult;

    fn filter_map(
        &self,
        extraction_results: &mut [ExtractorResult],
        service_index: &ServiceModelIndex,
    );

    /// Disambiguate extracted method calls
    fn disambiguate(
        &self,
        extraction_results: &mut [ExtractorResult],
        service_index: &ServiceModelIndex,
    );
}
