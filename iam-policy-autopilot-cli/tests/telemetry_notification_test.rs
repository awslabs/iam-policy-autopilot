use std::path::Path;
use std::sync::{Arc, Mutex};

use iam_policy_autopilot_common::telemetry::{
    TelemetryChoice, TelemetryConfig, TELEMETRY_ENV_VAR, TELEMETRY_NOTICE,
};
use rmcp::{
    handler::client::ClientHandler, model::LoggingMessageNotificationParam,
    service::NotificationContext, transport::TokioChildProcess, RmcpError, RoleClient, ServiceExt,
};
use rstest::rstest;
use tokio::process::Command;
use tokio::time::{sleep, Duration};

#[derive(Clone)]
struct NotificationCapture {
    messages: Arc<Mutex<Vec<LoggingMessageNotificationParam>>>,
}

impl NotificationCapture {
    fn new() -> Self {
        Self {
            messages: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn get_messages(&self) -> Vec<LoggingMessageNotificationParam> {
        self.messages.lock().unwrap().clone()
    }

    fn find_telemetry_notice(&self) -> Option<LoggingMessageNotificationParam> {
        self.get_messages()
            .into_iter()
            .find(|m| m.logger.as_deref() == Some("iam-policy-autopilot"))
    }
}

impl ClientHandler for NotificationCapture {
    fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        self.messages.lock().unwrap().push(params);
        std::future::ready(())
    }
}

async fn connect_mcp_client(
    home: &Path,
    env_override: Option<(&str, &str)>,
) -> (NotificationCapture, impl std::any::Any) {
    let capture = NotificationCapture::new();

    let mut command = Command::new(env!("CARGO_BIN_EXE_iam-policy-autopilot"));
    command
        .args(["mcp-server"])
        .env("HOME", home)
        .env_remove(TELEMETRY_ENV_VAR)
        .kill_on_drop(true);

    if let Some((key, value)) = env_override {
        command.env(key, value);
    }

    let transport = TokioChildProcess::new(command)
        .map_err(RmcpError::transport_creation::<TokioChildProcess>)
        .unwrap();

    let client = capture.clone().serve(transport).await.unwrap();

    // Wait for on_initialized to fire and the notification to arrive
    sleep(Duration::from_secs(3)).await;

    (capture, client)
}

#[rstest]
#[case::notice_sent_when_not_set(TelemetryChoice::NotSet, None, true)]
#[case::notice_suppressed_when_disabled(TelemetryChoice::Disabled, None, false)]
#[case::notice_suppressed_when_enabled(TelemetryChoice::Enabled, None, false)]
#[case::env_var_disables_overrides_config(TelemetryChoice::NotSet, Some((TELEMETRY_ENV_VAR, "true")), false)]
#[tokio::test]
async fn test_telemetry_notification(
    #[case] config_choice: TelemetryChoice,
    #[case] env_override: Option<(&str, &str)>,
    #[case] expect_notice: bool,
) {
    let home = tempfile::tempdir().unwrap();
    let config_dir = home.path().join(".iam-policy-autopilot");
    std::fs::create_dir(&config_dir).unwrap();
    let config = TelemetryConfig {
        installation_id: "00000000-0000-0000-0000-000000000000".into(),
        telemetry_choice: config_choice,
    };
    std::fs::write(
        config_dir.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();

    let (capture, _client) = connect_mcp_client(home.path(), env_override).await;
    let notice = capture.find_telemetry_notice();

    if expect_notice {
        let msg = notice.unwrap_or_else(|| {
            panic!(
                "Expected telemetry notice for config_choice={config_choice:?}, env_override={env_override:?}. \
                 Got messages: {:?}",
                capture.get_messages()
            )
        });

        assert_eq!(
            msg.level,
            rmcp::model::LoggingLevel::Notice,
            "Telemetry notice should use Notice level"
        );

        let data_str = msg
            .data
            .as_str()
            .expect("notification data should be a string");
        assert_eq!(
            data_str, TELEMETRY_NOTICE,
            "MCP notice content should match the shared TELEMETRY_NOTICE constant"
        );
    } else {
        assert!(
            notice.is_none(),
            "Should NOT receive telemetry notice for config_choice={config_choice:?}, \
             env_override={env_override:?}. Got: {:?}",
            capture.get_messages()
        );
    }
}
