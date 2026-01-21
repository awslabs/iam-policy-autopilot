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
