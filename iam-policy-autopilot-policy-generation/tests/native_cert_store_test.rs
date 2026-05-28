//! Integration test verifying that the HTTP client loads trusted CAs from the platform
//! rather than from a bundle compiled into the binary.
//!
//! We use `SSL_CERT_FILE` as a proxy for the OS certificate store in this test.
//! On Linux, `rustls-platform-verifier` delegates to `rustls-native-certs`, which
//! honors `SSL_CERT_FILE` / `SSL_CERT_DIR` as overrides for the system cert paths.
//! A hardcoded bundle (e.g. `webpki-roots`) would ignore these env vars entirely.
//!
//! The positive test injects a custom CA via `SSL_CERT_FILE` and verifies the CLI
//! trusts it. The negative test provides a wrong CA and verifies rejection.
//! Together they prove that trusted CAs come from `rustls-native-certs` (the platform
//! mechanism), not from a compiled-in bundle.
//!
//! This test only runs on Linux. On other platforms, `rustls-platform-verifier` uses
//! native APIs directly (CryptoAPI on Windows, Security.framework on macOS) with no
//! env var override available for testing.
#![cfg(target_os = "linux")]

use std::io::Write;
use std::net::SocketAddr;
use std::process::Output;
use std::sync::Arc;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::ServerConfig;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

fn mapping_response(server_url: &str) -> String {
    serde_json::json!([
        {"service": "s3", "url": format!("{server_url}/s3.json")}
    ])
    .to_string()
}

fn s3_service_reference_response() -> String {
    serde_json::json!({
        "Name": "s3",
        "Actions": [
            {
                "Name": "GetObject",
                "Resources": [{"Name": "object"}],
                "ActionConditionKeys": []
            }
        ],
        "Resources": [
            {
                "Name": "object",
                "ARNFormats": ["arn:${Partition}:s3:::${BucketName}/${ObjectName}"]
            }
        ],
        "Operations": [
            {
                "Name": "GetObject",
                "AuthorizedActions": [
                    {"Name": "GetObject", "Service": "s3"}
                ]
            }
        ]
    })
    .to_string()
}

fn generate_ca_and_server_cert(
    server_name: &str,
) -> (
    CertificateDer<'static>,
    CertificateDer<'static>,
    PrivateKeyDer<'static>,
) {
    let ca_key_pair = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_cert = ca_params.self_signed(&ca_key_pair).unwrap();
    let ca_issuer = Issuer::from_params(&ca_params, &ca_key_pair);

    let server_key_pair = KeyPair::generate().unwrap();
    let server_params = CertificateParams::new(vec![server_name.to_string()]).unwrap();
    let server_cert = server_params
        .signed_by(&server_key_pair, &ca_issuer)
        .unwrap();

    let ca_cert_der = CertificateDer::from(ca_cert.der().to_vec());
    let server_cert_der = CertificateDer::from(server_cert.der().to_vec());
    let server_key_der =
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key_pair.serialize_der()));

    (ca_cert_der, server_cert_der, server_key_der)
}

async fn start_tls_server(
    server_cert: CertificateDer<'static>,
    server_key: PrivateKeyDer<'static>,
) -> SocketAddr {
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![server_cert], server_key)
        .unwrap();

    let acceptor = TlsAcceptor::from(Arc::new(server_config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_url = format!("https://localhost:{}", addr.port());

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => continue,
            };
            let acceptor = acceptor.clone();
            let server_url = server_url.clone();
            tokio::spawn(async move {
                let Ok(tls_stream) = acceptor.accept(stream).await else {
                    return;
                };
                let io = TokioIo::new(tls_stream);
                let server_url = server_url.clone();
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(
                        io,
                        service_fn(move |req: Request<hyper::body::Incoming>| {
                            let server_url = server_url.clone();
                            async move {
                                let body = if req.uri().path() == "/s3.json" {
                                    s3_service_reference_response()
                                } else {
                                    mapping_response(&server_url)
                                };
                                Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from(body))))
                            }
                        }),
                    )
                    .await;
            });
        }
    });

    addr
}

/// Runs the CLI against the given TLS server address with the specified cert env vars.
/// `ssl_cert_file` controls what the client trusts; `ssl_cert_dir` overrides the system dir.
async fn run_cli_against_tls_server(
    addr: SocketAddr,
    ssl_cert_file: &std::path::Path,
    ssl_cert_dir: Option<&std::path::Path>,
) -> Output {
    let mut py_file = tempfile::NamedTempFile::with_suffix(".py").unwrap();
    py_file
        .write_all(
            b"import boto3\nclient = boto3.client('s3')\nclient.get_object(Bucket='b', Key='k')\n",
        )
        .unwrap();
    py_file.flush().unwrap();

    let cli_bin = assert_cmd::cargo::cargo_bin("iam-policy-autopilot");

    let mut cmd = std::process::Command::new(&cli_bin);
    cmd.args([
        "generate-policies",
        py_file.path().to_str().unwrap(),
        "--region",
        "us-east-1",
        "--account",
        "123456789012",
        "--disable-cache",
    ])
    .env("SSL_CERT_FILE", ssl_cert_file)
    .env(
        "IAM_POLICY_AUTOPILOT_SERVICE_REFERENCE_URL",
        format!("https://localhost:{}", addr.port()),
    )
    .env("DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY", "true")
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());

    if let Some(dir) = ssl_cert_dir {
        cmd.env("SSL_CERT_DIR", dir);
    }

    let child = cmd.spawn().expect("failed to spawn CLI binary");

    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::task::spawn_blocking(move || child.wait_with_output().unwrap())
            .await
            .unwrap()
    })
    .await
    .expect("CLI timed out after 30s — likely a TLS handshake hang")
}

/// With the custom CA in `SSL_CERT_FILE`, the CLI should successfully connect
/// to our TLS server and produce a valid IAM policy.
#[tokio::test]
async fn test_native_cert_store_with_cli() {
    let (ca_cert_der, server_cert_der, server_key_der) = generate_ca_and_server_cert("localhost");
    let addr = start_tls_server(server_cert_der, server_key_der).await;

    let ca_pem = pem::encode(&pem::Pem::new("CERTIFICATE", ca_cert_der.to_vec()));
    let mut cert_file = tempfile::NamedTempFile::new().unwrap();
    cert_file.write_all(ca_pem.as_bytes()).unwrap();
    cert_file.flush().unwrap();

    let output = run_cli_against_tls_server(addr, cert_file.path(), None).await;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "CLI failed. This likely means the native certificate store is not being respected.\n\
         stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("s3:GetObject"),
        "Expected policy output with s3:GetObject action.\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// With the wrong CA in the trust store, the CLI should fail with a certificate error.
/// This proves the positive test above is meaningful — TLS verification is real.
#[tokio::test]
async fn test_native_cert_store_fails_with_wrong_ca() {
    let (_ca_cert_der, server_cert_der, server_key_der) = generate_ca_and_server_cert("localhost");
    let addr = start_tls_server(server_cert_der, server_key_der).await;

    // Generate a different CA that did NOT sign the server cert
    let wrong_ca_key = KeyPair::generate().unwrap();
    let mut wrong_ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    wrong_ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let wrong_ca_cert = wrong_ca_params.self_signed(&wrong_ca_key).unwrap();
    let wrong_ca_pem = pem::encode(&pem::Pem::new("CERTIFICATE", wrong_ca_cert.der().to_vec()));

    let mut wrong_cert_file = tempfile::NamedTempFile::new().unwrap();
    wrong_cert_file.write_all(wrong_ca_pem.as_bytes()).unwrap();
    wrong_cert_file.flush().unwrap();

    // Use the wrong CA as the only trusted cert, with an empty cert dir
    // to prevent the system store from being used
    let empty_cert_dir = tempfile::tempdir().unwrap();

    let output =
        run_cli_against_tls_server(addr, wrong_cert_file.path(), Some(empty_cert_dir.path())).await;

    let stderr = String::from_utf8_lossy(&output.stderr);

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !output.status.success(),
        "CLI should have failed with the wrong CA in the trust store.\n\
         stdout: {stdout}\nstderr: {stderr}"
    );

    let url = format!("https://localhost:{}", addr.port());
    let expected_error = format!(
        "Error: Enrichment failed for method 'get_object' (service 's3'): \
         Network error: Failed to connect to the service reference endpoint at '{url}'. \
         Verify that this URL is reachable from your network \
         (e.g. not blocked by a firewall or VPN). \
         If you need to route through a proxy, \
         set the HTTPS_PROXY environment variable \
         (see https://docs.rs/reqwest/latest/reqwest/#proxies)"
    );
    assert!(
        stderr.lines().any(|line| line == expected_error),
        "Expected exact error line in stderr.\nExpected: {expected_error}\nGot: {stderr}"
    );
}
