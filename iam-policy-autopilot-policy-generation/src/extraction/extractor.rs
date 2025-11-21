//! Result type alias for operations that can fail with `ExtractorError`
use ast_grep_core::AstGrep;
use ast_grep_language::{Go, JavaScript, Python, TypeScript};
use async_trait::async_trait;

use crate::extraction::go::types::GoImportInfo;
use crate::{SdkMethodCall, ServiceModelIndex};

use std::fmt::Debug;

/// Enum to handle different AST types from different languages
#[derive(Clone)]
pub(crate) enum ExtractorResult {
    Python(
        AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
        Vec<SdkMethodCall>,
    ),
    Go(
        AstGrep<ast_grep_core::tree_sitter::StrDoc<Go>>,
        Vec<SdkMethodCall>,
        GoImportInfo,
    ),
    JavaScript(
        AstGrep<ast_grep_core::tree_sitter::StrDoc<JavaScript>>,
        Vec<SdkMethodCall>,
    ),
    TypeScript(
        AstGrep<ast_grep_core::tree_sitter::StrDoc<TypeScript>>,
        Vec<SdkMethodCall>,
    ),
}

impl Debug for ExtractorResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Python(_arg0, arg1) => f.debug_tuple("Python").field(arg1).finish(),
            Self::Go(_arg0, arg1, arg2) => f.debug_tuple("Go").field(arg1).field(arg2).finish(),
            Self::JavaScript(_arg0, arg1) => f.debug_tuple("JavaScript").field(arg1).finish(),
            Self::TypeScript(_arg0, arg1) => f.debug_tuple("TypeScript").field(arg1).finish(),
        }
    }
}

impl ExtractorResult {
    /// Extract just the method calls from the result
    pub(crate) fn method_calls(self) -> Vec<SdkMethodCall> {
        match self {
            ExtractorResult::Python(_, calls) => calls,
            ExtractorResult::Go(_, calls, _) => calls,
            ExtractorResult::JavaScript(_, calls) => calls,
            ExtractorResult::TypeScript(_, calls) => calls,
        }
    }

    /// Get a reference to the method calls without consuming the result
    #[allow(dead_code)]
    pub(crate) fn method_calls_ref(&self) -> &Vec<SdkMethodCall> {
        match self {
            ExtractorResult::Python(_, calls) => calls,
            ExtractorResult::Go(_, calls, _) => calls,
            ExtractorResult::JavaScript(_, calls) => calls,
            ExtractorResult::TypeScript(_, calls) => calls,
        }
    }

    /// Get a reference to the import information for Go results
    #[allow(dead_code)]
    pub(crate) fn go_import_info(&self) -> Option<&GoImportInfo> {
        match self {
            ExtractorResult::Go(_, _, import_info) => Some(import_info),
            _ => None,
        }
    }
}

/// Extractor trait
#[async_trait]
pub(crate) trait Extractor: Send + Sync {
    /// Parse source code into method calls and return the AST
    async fn parse(&self, source_code: &str) -> ExtractorResult;

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
