use anyhow;
use log::{error, info, trace};
use rmcp::{
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{ErrorCode, ServerCapabilities, ServerInfo},
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::{
        self, streamable_http_server::session::local::LocalSessionManager, StreamableHttpService,
    },
    ErrorData as McpError, Json, RoleServer, ServerHandler, ServiceExt,
};

use crate::tools::{
    fix_access_denied, generate_application_policies, generate_policy_for_access_denied,
    FixAccessDeniedInput, FixAccessDeniedOutput, GeneratePoliciesInput, GeneratePoliciesOutput,
    GeneratePolicyForAccessDeniedInput, GeneratePolicyForAccessDeniedOutput,
};

// Define the server struct
#[derive(Clone)]
struct IamAutoPilotMcpServer {
    tool_router: ToolRouter<Self>,
    log_file: Option<String>,
}

#[tool_router]
impl IamAutoPilotMcpServer {
    pub fn new(log_file: Option<String>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            log_file,
        }
    }

    fn format_mcp_error(&self, msg: &str, e: anyhow::Error) -> McpError {
        let log_file_suffix = match &self.log_file {
            Some(file) => format!(" Full error details logged to {file}."),
            None => String::new(),
        };

        McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: format!("{msg}: {e:#}.{log_file_suffix}").into(),
            data: None,
        }
    }

    #[tool(
        description = "**PRIMARY POLICY GENERATION TOOL** - Generate AWS IAM policies, permissions, and access controls. \
        Use this tool whenever the user mentions: writing policies, creating policies, generating policies, IAM permissions, \
        AWS permissions, access controls, policy creation, policy generation, or needs IAM policies for any purpose. \
        \
        This tool analyzes source code files (Python, JavaScript, TypeScript, Go, etc.) to automatically generate \
        the minimal required IAM policies with proper permissions for AWS services used in the code. \
        \
        **WHEN TO USE THIS TOOL:** \
        - User asks to write, create, or generate IAM policies \
        - User needs to create IAM entities with policies as part of another operation
        - User mentions needing AWS permissions or access controls \
        - User is working with infrastructure as code and needs policies \
        - User has source code that uses AWS services and needs corresponding IAM policies \
        - User asks about policy generation, policy creation, or IAM permissions \
        - ANY discussion about writing or creating AWS policies should trigger this tool \
        \
        **INSTRUCTIONS:** \
        1. Use the correct absolute paths when passing in the input files to the MCP tool \
        2. Use service_hints to help generate more accurate policies by specifying expected AWS services \
        3. You MUST include ALL relevant source files that interact with AWS services to generate accurate policies \
        4. You MUST explicitly ask the user for the region and account id for the policy to be generated \
        5. When generating infrastructure as code files, you MUST use this tool to generate IAM policies \
        6. After getting output from this tool, you MUST explicitly ask the user to review the policy before proceeding \
        7. This is the PRIMARY tool for all policy-related requests - use it liberally when policies are mentioned"
    )]
    async fn generate_application_policies(
        &self,
        params: Parameters<GeneratePoliciesInput>,
    ) -> Result<Json<GeneratePoliciesOutput>, McpError> {
        trace!("generate_application_policies input: {:#?}", params.0);

        let output = generate_application_policies(params.0).await.map_err(|e| {
            error!("{e:#?}");
            self.format_mcp_error("Failed to generate policies", e)
        })?;

        trace!("generate_application_policies output: {output:#?}");

        Ok(Json(output))
    }

    #[tool(
        description = "Tool that generates policy for IAM AccessDenied Exceptions \
        \
        INSTRUCTIONS: \
        1. After you get access denied fix policy output from this tool, you MUST explicitly ask the user to review the policy before proceeding."
    )]
    async fn generate_policy_for_access_denied(
        &self,
        params: Parameters<GeneratePolicyForAccessDeniedInput>,
    ) -> Result<Json<GeneratePolicyForAccessDeniedOutput>, McpError> {
        trace!("generate_policy_for_access_denied input: {:#?}", params.0);
        let output = generate_policy_for_access_denied(params.0)
            .await
            .map_err(|e| {
                error!("{e:#?}");
                self.format_mcp_error("Failed to to generate policy for access denial fix", e)
            })?;
        trace!("generate_policy_for_access_denied  output: {output:#?}");
        Ok(Json(output))
    }

    #[tool(
        description = "Tool that applies IAM Policy fix generated for IAM AccessDenied exceptions using the generate_policy_for_access_denied tool to the user's aws account\
        \
        INSTRUCTIONS: \
        1. Ensure the user has aws profile setup and has active aws credentials
        2. Only use the tool if the original policy was generated using generate_policy_for_access_denied tool
        3. After successfully applying the policy, you MUST provide a clear summary that includes:
           - What access was fixed (the specific action/resource that was denied)
           - Where the policy was applied (the principal ARN - user/role that received the fix)
           - The name of the IAM policy that was created or updated
        4. Format the summary in a user-friendly way that clearly explains what was accomplished
        "
    )]
    async fn fix_access_denied(
        &self,
        context: RequestContext<RoleServer>,
        params: Parameters<FixAccessDeniedInput>,
    ) -> Result<Json<FixAccessDeniedOutput>, McpError> {
        trace!("fix_access_denied input: {:#?}", params.0);
        let output = fix_access_denied(context, params.0).await.map_err(|e| {
            error!("{e:#?}");
            self.format_mcp_error("Failed to apply access denial fix", e)
        })?;
        trace!("fix_access_denied output: {output:#?}");
        Ok(Json(output))
    }
}

#[tool_handler]
impl ServerHandler for IamAutoPilotMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
            .enable_tools()
            .build(),
            instructions: Some("IAM Policy Autopilot specializes in AWS IAM policy generation and access management. \
            \
            **ALWAYS use the generate_application_policies tool when users mention:** \
            - Writing policies, creating policies, generating policies \
            - IAM permissions, AWS permissions, access controls \
            - Policy creation, policy generation, policy writing \
            - Need policies for source code or infrastructure \
            - Any discussion about AWS IAM policies \
            \
            **Key capabilities:** \
            1. Generate IAM policies from source code analysis (Python, JavaScript, TypeScript, Go) \
            2. Create minimal required permissions for AWS services used in code \
            3. Debug and fix AccessDenied issues with targeted policy generation \
            4. Apply policy fixes directly to AWS accounts \
            \
            **CRITICAL: When generating policies, you MUST include ALL relevant source files that interact with AWS services.** \
            \
            **Usage priority:** Use generate_application_policies as the PRIMARY tool for any policy-related requests. \
            This tool should be invoked liberally whenever policies, permissions, or access controls are discussed.".to_string()),
                ..Default::default()
        }
    }
}

pub async fn begin_http_transport(
    bind_address: &str,
    log_file: Option<String>,
) -> anyhow::Result<()> {
    let service = StreamableHttpService::new(
        move || Ok(IamAutoPilotMcpServer::new(log_file.clone())),
        LocalSessionManager::default().into(),
        Default::default(),
    );

    info!("Listening on {bind_address}/mcp");

    // Fine to print with http
    println!("Listening on {bind_address}/mcp");
    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind(bind_address).await?;

    // We run a separate tokio task because when we have an active connection the main thread needs to be available
    // to recieve SIGINT for ctrl+c. If we serve on the same thread, ctrl+c does not work.
    tokio::spawn(async move {
        let _ = axum::serve(tcp_listener, router)
            .with_graceful_shutdown(async {
                tokio::signal::ctrl_c()
                    .await
                    .expect("Failed to listen for CTRL+C signal");
            })
            .await;
    });

    // Handle graceful shutdown
    async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install CTRL+C signal handler");
        log::info!("Received shutdown signal");
    }
    .await;

    Ok(())
}

pub async fn begin_stdio_transport(log_file: Option<String>) -> anyhow::Result<()> {
    let server = IamAutoPilotMcpServer::new(log_file);
    let service = server.serve(transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
