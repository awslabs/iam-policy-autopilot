use std::process::Command;

// Test error text constants
const IMPLICIT_DENY: &str = "User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key because no identity-based policy allows the s3:GetObject action";

const RESOURCE_POLICY: &str = "User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key because no resource-based policy allows the s3:GetObject action";

const EXPLICIT_DENY: &str = "User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key with an explicit deny";

const OTHER_DENIAL: &str = "User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key because of a service control policy";

const INVALID_INPUT: &str = "Random error message without AccessDenied pattern";

#[test]
fn help_shows_no_features() {
    let out = Command::new(env!("CARGO_BIN_EXE_iam-policy-autopilot"))
        .arg("--help")
        .output()
        .expect("failed to run --help");
    let s = String::from_utf8_lossy(&out.stdout);
    // Should not mention features in the simplified version
    assert!(
        !s.contains("Enabled features:"),
        "help should not mention features: {}",
        s
    );
}

#[test]
fn test_fix_access_denied_implicit_deny() {
    let output = Command::new(env!("CARGO_BIN_EXE_iam-policy-autopilot"))
        .args(["fix-access-denied", IMPLICIT_DENY])
        .output()
        .expect("failed to run fix-access-denied");

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should contain plan details and refuse to apply without TTY
    assert!(
        stderr.contains("s3:GetObject") || stderr.contains("Action"),
        "stderr was: {}",
        stderr
    );
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn test_fix_access_denied_resource_policy() {
    let output = Command::new(env!("CARGO_BIN_EXE_iam-policy-autopilot"))
        .args(["fix-access-denied", RESOURCE_POLICY])
        .output()
        .expect("failed to run fix-access-denied with resource policy");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should detect ResourcePolicy denial and show JSON statement
    assert_eq!(output.status.code(), Some(2)); // Manual action required
    assert!(
        stderr.contains("ResourcePolicy") || stderr.contains("resource-based policy"),
        "stderr was: {}",
        stderr
    );
}

#[test]
fn test_fix_access_denied_explicit_deny() {
    let output = Command::new(env!("CARGO_BIN_EXE_iam-policy-autopilot"))
        .args(["fix-access-denied", EXPLICIT_DENY])
        .output()
        .expect("failed to run fix-access-denied with explicit deny");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should detect ExplicitIdentity denial and explain limitation
    assert_eq!(output.status.code(), Some(2)); // Cannot fix
    assert!(
        stderr.contains("explicit deny") || stderr.contains("ExplicitIdentity"),
        "stderr was: {}",
        stderr
    );
}

#[test]
fn test_fix_access_denied_other_denial() {
    let output = Command::new(env!("CARGO_BIN_EXE_iam-policy-autopilot"))
        .args(["fix-access-denied", OTHER_DENIAL])
        .output()
        .expect("failed to run fix-access-denied with other denial");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should detect Other denial type and show unsupported message
    assert_eq!(output.status.code(), Some(2)); // Cannot fix
    assert!(
        stderr.contains("not supported") || stderr.contains("Other"),
        "stderr was: {}",
        stderr
    );
}

#[test]
fn test_fix_access_denied_invalid_input() {
    let output = Command::new(env!("CARGO_BIN_EXE_iam-policy-autopilot"))
        .args(["fix-access-denied", INVALID_INPUT])
        .output()
        .expect("failed to run fix-access-denied with invalid input");

    assert_eq!(output.status.code(), Some(2));

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should indicate no AccessDenied found
    assert!(
        stderr.contains("No AccessDenied found"),
        "stderr was: {}",
        stderr
    );
}

#[test]
fn test_fix_access_denied_with_yes_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_iam-policy-autopilot"))
        .args(["fix-access-denied", IMPLICIT_DENY, "--yes"])
        .output()
        .expect("failed to run fix-access-denied with --yes");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // With --yes flag, should attempt to apply (but will likely fail with AWS credentials in test)
    // We just verify it doesn't refuse with TTY message
    assert!(
        !stderr.contains("run interactively in a TTY")
            || stderr.contains("Failed to initialize service"),
        "stderr was: {}",
        stderr
    );
}

#[test]
fn test_fix_access_denied_stdin_input() {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_iam-policy-autopilot"))
        .args(["fix-access-denied"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn command");

    {
        let stdin = child.stdin.as_mut().expect("failed to get stdin");
        stdin
            .write_all(IMPLICIT_DENY.as_bytes())
            .expect("failed to write to stdin");
    } // stdin reference dropped here, then we take ownership and drop it
    drop(child.stdin.take()); // Close stdin to signal EOF

    let output = child.wait_with_output().expect("failed to wait for child");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should parse and show plan
    assert!(
        stderr.contains("s3:GetObject") || stderr.contains("Action"),
        "stderr was: {}",
        stderr
    );
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn test_mcp_server_help_shows_bind_address() {
    let out = Command::new(env!("CARGO_BIN_EXE_iam-policy-autopilot"))
        .args(["mcp-server", "--help"])
        .output()
        .expect("failed to run mcp-server --help");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--bind-address"),
        "mcp-server help should mention --bind-address: {}",
        stdout
    );
}

#[test]
fn test_mcp_server_rejects_unknown_args() {
    let output = Command::new(env!("CARGO_BIN_EXE_iam-policy-autopilot"))
        .args(["mcp-server", "--unknown-flag"])
        .output()
        .expect("failed to run mcp-server with unknown flag");
    assert!(!output.status.success(), "should fail with unknown flag");
}

#[test]
fn test_mcp_server_bind_address_accepts_custom_value() {
    // Start with an invalid address to quickly verify the argument is parsed and passed through.
    // The server should attempt to bind and fail, proving the argument was consumed.
    let output = Command::new(env!("CARGO_BIN_EXE_iam-policy-autopilot"))
        .args([
            "mcp-server",
            "--transport",
            "http",
            "--port",
            "0",
            "--bind-address",
            "192.0.2.1", // TEST-NET address, should fail to bind
        ])
        .output()
        .expect("failed to run mcp-server with custom bind address");

    let stderr = String::from_utf8_lossy(&output.stderr);
    // The server should have attempted to bind to the custom address (and failed),
    // proving the argument was parsed and forwarded. Exit code should be non-zero.
    assert!(
        !output.status.success(),
        "server should fail to bind to TEST-NET address, stderr: {}",
        stderr
    );
}
