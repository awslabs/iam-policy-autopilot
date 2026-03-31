//! CLI-specific type definitions.
//!
//! This module contains types that are specific to the CLI binary and should
//! not be part of the core library.

/// Exit codes for the CLI application.
///
/// These codes follow the documented convention where:
/// - 0 indicates successful completion
/// - 1 indicates duplicate statement (operation succeeded but no changes made)
/// - 2 indicates failure, refusal, or manual action required
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// Operation completed successfully
    Success,

    /// Duplicate statement - permission already exists in policy
    Duplicate,

    /// Error, validation failure, or manual action required
    Error,
}

impl ExitCode {
    /// Convert to the integer exit code for process::exit()
    pub fn code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Duplicate => 1,
            Self::Error => 2,
        }
    }
}

impl From<ExitCode> for i32 {
    fn from(exit_code: ExitCode) -> Self {
        exit_code.code()
    }
}
