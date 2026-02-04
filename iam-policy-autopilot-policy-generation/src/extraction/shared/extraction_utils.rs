use crate::extraction::core::Parameter;
use crate::Location;

/// Information about a discovered waiter creation call (get_waiter in Python, NewXxxWaiter in Go)
#[derive(Debug, Clone)]
pub(crate) struct WaiterCreationInfo {
    /// Variable name assigned to the waiter (e.g., "waiter", "instance_waiter")
    pub variable_name: String,
    /// Waiter name in standardized format (e.g., "instance_terminated")
    pub waiter_name: String,
    /// Client receiver variable name (e.g., "client", "ec2_client")
    pub client_receiver: String,
    /// Location of the waiter creation call
    pub location: Location,
    /// The expression text of the waiter creation call
    pub expr: String,
}

/// Information about a wait method call (wait() in Python, Wait() in Go)
#[derive(Debug, Clone)]
pub(crate) struct WaiterCallInfo {
    /// Waiter variable being called (e.g., "waiter")
    pub waiter_var: String,
    /// Extracted arguments (language-specific filtering applied)
    pub arguments: Vec<Parameter>,
    /// Location of the wait call node
    pub location: Location,
    /// The expression text of the wait call
    pub expr: String,
}

/// Information about a chained waiter call (client.get_waiter().wait() - Python only)
#[derive(Debug, Clone)]
pub(crate) struct ChainedWaiterCallInfo {
    /// Client receiver variable name (e.g., "dynamodb_client")
    pub client_receiver: String,
    /// Waiter name in standardized format (e.g., "table_exists")
    pub waiter_name: String,
    /// Extracted arguments from wait call
    pub arguments: Vec<Parameter>,
    /// Location of the chained call node
    pub location: Location,
    /// The expression text of the chained call
    pub expr: String,
}

/// Information about a discovered paginator creation call (get_paginator in Python, NewXxxPaginator in Go)
#[derive(Debug, Clone)]
pub(crate) struct PaginatorCreationInfo {
    /// Variable name assigned to the paginator (e.g., "paginator", "list_paginator")
    pub variable_name: String,
    /// Operation name in standardized format (e.g., "list_objects_v2")
    pub operation_name: String,
    /// Client receiver variable name (e.g., "client", "s3_client")
    pub client_receiver: String,
    /// Location of the paginator creation call
    pub location: Location,
    /// Extracted arguments from paginator creation (Go only - input struct)
    /// For Python, this is typically empty as arguments come from paginate() call
    pub creation_arguments: Vec<Parameter>,
    /// The expression text of the paginator creation call
    pub expr: String,
}

/// Information about a paginate method call (paginate() in Python, Pages() in Go)
#[derive(Debug, Clone)]
pub(crate) struct PaginatorCallInfo {
    /// Paginator variable being called (e.g., "paginator")
    pub paginator_var: String,
    /// Extracted arguments (language-specific filtering applied)
    pub arguments: Vec<Parameter>,
    /// Location of the paginate call node
    pub location: Location,
    /// The expression text of the paginate call
    pub expr: String,
}

/// Information about a chained paginator call (client.get_paginator().paginate() - Python only)
#[derive(Debug, Clone)]
pub(crate) struct ChainedPaginatorCallInfo {
    /// Client receiver variable name (e.g., "s3_client")
    pub client_receiver: String,
    /// Operation name in standardized format (e.g., "list_objects_v2")
    pub operation_name: String,
    /// Extracted arguments from paginate call
    pub arguments: Vec<Parameter>,
    /// Location of the chained call node
    pub location: Location,
    /// The expression text of the chained call
    pub expr: String,
}

/// Unified representation of waiter call information across different patterns
///
/// This enum abstracts over the three common waiter call scenarios:
/// 1. CreationOnly - Waiter created but no wait call found
/// 2. Matched - Waiter creation + wait call matched
/// 3. Chained - Direct chained call (Python-specific)
///
/// The enum provides helper methods to extract common data regardless of variant,
/// enabling shared synthetic call creation logic across Python and Go extractors.
#[derive(Debug, Clone)]
pub(crate) enum WaiterCallPattern<'a> {
    /// Waiter creation without matched wait call
    ///
    /// Used when we find `get_waiter()` or `NewXxxWaiter()` but no corresponding wait.
    /// Synthetic calls use required parameters from service model.
    CreationOnly(&'a WaiterCreationInfo),

    /// Matched waiter creation + wait call
    ///
    /// Used when we find both creation and wait, and can match them.
    /// Synthetic calls use arguments from the wait call.
    Matched {
        creation: &'a WaiterCreationInfo,
        wait: &'a WaiterCallInfo,
    },

    /// Chained waiter call (Python-specific)
    ///
    /// Used for `client.get_waiter('name').wait(args)` pattern.
    /// Synthetic calls use arguments from the chained call.
    ///
    /// Note: Go SDK does not support this pattern, so Go extractors
    /// will never construct this variant.
    Chained(&'a ChainedWaiterCallInfo),
}

impl<'a> WaiterCallPattern<'a> {
    /// Get the waiter name from any variant
    pub(crate) fn waiter_name(&self) -> &'a str {
        match self {
            Self::CreationOnly(info) | Self::Matched { creation: info, .. } => &info.waiter_name,
            Self::Chained(info) => &info.waiter_name,
        }
    }

    /// Get the expression text from any variant
    ///
    /// For Matched variant, returns the wait call expression (most specific).
    pub(crate) fn expr(&self) -> &'a str {
        match self {
            Self::CreationOnly(info) => &info.expr,
            Self::Matched { wait, .. } => &wait.expr,
            Self::Chained(info) => &info.expr,
        }
    }

    /// Get the location from any variant
    ///
    /// For Matched variant, returns the wait call location (most specific).
    pub(crate) fn location(&self) -> &'a Location {
        match self {
            Self::CreationOnly(info) => &info.location,
            Self::Matched { wait, .. } => &wait.location,
            Self::Chained(info) => &info.location,
        }
    }

    /// Get the client receiver from any variant
    pub(crate) fn client_receiver(&self) -> &'a str {
        match self {
            Self::CreationOnly(info) | Self::Matched { creation: info, .. } => {
                &info.client_receiver
            }
            Self::Chained(info) => &info.client_receiver,
        }
    }

    /// Get arguments if available (None for CreationOnly)
    ///
    /// Returns:
    /// - None for CreationOnly (must use required parameters from service model)
    /// - Some(&[Parameter]) for Matched and Chained variants
    pub(crate) fn arguments(&self) -> Option<&'a [Parameter]> {
        match self {
            Self::CreationOnly(_) => None,
            Self::Matched { wait, .. } => Some(&wait.arguments),
            Self::Chained(info) => Some(&info.arguments),
        }
    }
}
