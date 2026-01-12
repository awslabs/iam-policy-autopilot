use crate::extraction::core::Parameter;

/// Information about a discovered waiter creation call (get_waiter in Python, NewXxxWaiter in Go)
#[derive(Debug, Clone)]
pub struct WaiterCreationInfo {
    /// Variable name assigned to the waiter (e.g., "waiter", "instance_waiter")
    pub variable_name: String,
    /// Waiter name in standardized format (e.g., "instance_terminated")
    pub waiter_name: String,
    /// Client receiver variable name (e.g., "client", "ec2_client")
    pub client_receiver: String,
    /// Start position of the waiter creation call
    pub start_position: (usize, usize),
    /// End position of the waiter creation call
    pub end_position: (usize, usize),
}

impl WaiterCreationInfo {
    /// Get the line number from the start position
    pub fn line(&self) -> usize {
        self.start_position.0
    }
}

/// Information about a wait method call (wait() in Python, Wait() in Go)
#[derive(Debug, Clone)]
pub struct WaiterCallInfo {
    /// Waiter variable being called (e.g., "waiter")
    pub waiter_var: String,
    /// Extracted arguments (language-specific filtering applied)
    pub arguments: Vec<Parameter>,
    /// Start position of the wait call node
    pub start_position: (usize, usize),
    /// End position of the wait call node
    pub end_position: (usize, usize),
}

impl WaiterCallInfo {
    /// Get the line number from the start position
    pub fn line(&self) -> usize {
        self.start_position.0
    }
}

/// Information about a chained waiter call (client.get_waiter().wait() - Python only)
#[derive(Debug, Clone)]
pub struct ChainedWaiterCallInfo {
    /// Client receiver variable name (e.g., "dynamodb_client")
    pub client_receiver: String,
    /// Waiter name in standardized format (e.g., "table_exists")
    pub waiter_name: String,
    /// Extracted arguments from wait call
    pub arguments: Vec<Parameter>,
    /// Start position of the chained call node
    pub start_position: (usize, usize),
    /// End position of the chained call node
    pub end_position: (usize, usize),
}

/// Information about a discovered paginator creation call (get_paginator in Python, NewXxxPaginator in Go)
#[derive(Debug, Clone)]
pub struct PaginatorCreationInfo {
    /// Variable name assigned to the paginator (e.g., "paginator", "list_paginator")
    pub variable_name: String,
    /// Operation name in standardized format (e.g., "list_objects_v2")
    pub operation_name: String,
    /// Client receiver variable name (e.g., "client", "s3_client")
    pub client_receiver: String,
    /// Start position of the paginator creation call
    pub start_position: (usize, usize),
    /// End position of the paginator creation call
    pub end_position: (usize, usize),
    /// Extracted arguments from paginator creation (Go only - input struct)
    /// For Python, this is typically empty as arguments come from paginate() call
    pub creation_arguments: Vec<Parameter>,
}

impl PaginatorCreationInfo {
    /// Get the line number from the start position
    pub fn line(&self) -> usize {
        self.start_position.0
    }
}

/// Information about a paginate method call (paginate() in Python, Pages() in Go)
#[derive(Debug, Clone)]
pub struct PaginatorCallInfo {
    /// Paginator variable being called (e.g., "paginator")
    pub paginator_var: String,
    /// Extracted arguments (language-specific filtering applied)
    pub arguments: Vec<Parameter>,
    /// Start position of the paginate call node
    pub start_position: (usize, usize),
    /// End position of the paginate call node
    pub end_position: (usize, usize),
}

impl PaginatorCallInfo {
    /// Get the line number from the start position
    pub fn line(&self) -> usize {
        self.start_position.0
    }
}

/// Information about a chained paginator call (client.get_paginator().paginate() - Python only)
#[derive(Debug, Clone)]
pub struct ChainedPaginatorCallInfo {
    /// Client receiver variable name (e.g., "s3_client")
    pub client_receiver: String,
    /// Operation name in standardized format (e.g., "list_objects_v2")
    pub operation_name: String,
    /// Extracted arguments from paginate call
    pub arguments: Vec<Parameter>,
    /// Start position of the chained call node
    pub start_position: (usize, usize),
    /// End position of the chained call node
    pub end_position: (usize, usize),
}
