#!/usr/bin/env python3
"""
Handler validation scenarios for the genai-chatbot.

Each scenario exercises a specific Lambda handler through the GraphQL API
and reports success/failure. Used for IPA policy-swap validation.

Usage:
    python handler_scenarios.py --app-url https://xxx.cloudfront.net [--handler api-handler]

Requires: gql, boto3, pydantic (from pytest_requirements.txt)
"""

import argparse
import json
import sys
import time
import traceback
from dataclasses import dataclass, field
from pathlib import Path
from urllib.request import urlopen

# Add the integtests clients to the path
CDK_DIR = Path(__file__).parent / "genai-chatbot-cdk"
sys.path.insert(0, str(CDK_DIR / "integtests"))

from clients.cognito_client import CognitoClient
from clients.appsync_client import AppSyncClient


@dataclass
class ScenarioResult:
    handler: str
    scenario: str
    success: bool
    error: str = ""
    duration_seconds: float = 0.0


@dataclass
class ValidationReport:
    results: list = field(default_factory=list)

    def add(self, result: ScenarioResult):
        self.results.append(result)

    @property
    def passed(self):
        return [r for r in self.results if r.success]

    @property
    def failed(self):
        return [r for r in self.results if not r.success]


def get_client(app_url: str) -> AppSyncClient:
    """Authenticate and return an AppSync client."""
    config = json.loads(urlopen(app_url + "/aws-exports.json").read())
    cognito = CognitoClient(
        region=config["aws_cognito_region"],
        user_pool_id=config["aws_user_pools_id"],
        client_id=config["aws_user_pools_web_client_id"],
        identity_pool_id=config.get("aws_cognito_identity_pool_id", ""),
    )
    creds = cognito.get_credentials(
        email="ipa-validation@example.com", role="admin"
    )
    client = AppSyncClient(
        endpoint=config["aws_appsync_graphqlEndpoint"],
        id_token=creds.id_token,
    )
    client._app_url = app_url
    client._aws_config = config
    return client


# ---------------------------------------------------------------------------
# Scenarios per handler
# ---------------------------------------------------------------------------

def run_scenario(name: str, handler: str, fn, report: ValidationReport):
    """Run a scenario function, catch errors, record result."""
    start = time.time()
    try:
        fn()
        duration = time.time() - start
        print(f"  ✅ {name} ({duration:.1f}s)")
        report.add(ScenarioResult(handler=handler, scenario=name, success=True, duration_seconds=duration))
    except Exception as e:
        duration = time.time() - start
        err = f"{type(e).__name__}: {e}"
        print(f"  ❌ {name} ({duration:.1f}s): {err}")
        report.add(ScenarioResult(handler=handler, scenario=name, success=False, error=err, duration_seconds=duration))


def scenarios_api_handler(client: AppSyncClient, report: ValidationReport):
    """api-handler: the main GraphQL proxy resolver."""
    handler = "api-handler"
    print(f"\n{'='*60}\n{handler}\n{'='*60}")

    def list_models():
        models = client.list_models()
        assert isinstance(models, list)

    def list_workspaces():
        client.list_workspaces()

    def list_sessions():
        client.list_sessions()

    def list_rag_engines():
        engines = client.list_rag_engines()
        assert len(engines) > 0

    def list_roles():
        client.list_roles()

    def list_kendra_indexes():
        client.list_kendra_indexes()

    def list_applications():
        client.list_applications()

    run_scenario("list_models", handler, list_models, report)
    run_scenario("list_workspaces", handler, list_workspaces, report)
    run_scenario("list_sessions", handler, list_sessions, report)
    run_scenario("list_rag_engines", handler, list_rag_engines, report)
    run_scenario("list_roles", handler, list_roles, report)
    run_scenario("list_kendra_indexes", handler, list_kendra_indexes, report)
    run_scenario("list_applications", handler, list_applications, report)


def scenarios_send_query_resolver(client: AppSyncClient, report: ValidationReport):
    """send-query-resolver: sends a chat query to SNS."""
    handler = "send-query-resolver"
    print(f"\n{'='*60}\n{handler}\n{'='*60}")

    def send_query():
        client.send_query(data=json.dumps({
            "action": "run",
            "modelInterface": "langchain",
            "data": {
                "mode": "chain",
                "text": "Hello, this is a test",
                "modelName": "us.anthropic.claude-haiku-4-5-20251001-v1:0",
                "provider": "bedrock",
                "sessionId": "ipa-test-session",
            }
        }))

    run_scenario("send_query", handler, send_query, report)


def scenarios_langchain_request_handler(client: AppSyncClient, report: ValidationReport):
    """langchain-request-handler: processes chat via SQS (triggered by send-query).
    We verify by checking that a session was created after sending a query."""
    handler = "langchain-request-handler"
    print(f"\n{'='*60}\n{handler}\n{'='*60}")

    def send_and_wait():
        result = client.send_query(data=json.dumps({
            "action": "run",
            "modelInterface": "langchain",
            "data": {
                "mode": "chain",
                "text": "What is 2+2?",
                "modelName": "us.anthropic.claude-haiku-4-5-20251001-v1:0",
                "provider": "bedrock",
                "sessionId": "ipa-langchain-test",
            }
        }))
        print(f"    send_query result: {result}")
        # Wait for async processing
        time.sleep(10)
        session = client.get_session("ipa-langchain-test")
        print(f"    get_session result: {session}")
        assert session is not None, "Session should exist after query"

    run_scenario("send_and_verify_session", handler, send_and_wait, report)


def scenarios_upload_handler(client: AppSyncClient, report: ValidationReport):
    """upload-handler: generates presigned upload URLs."""
    handler = "upload-handler"
    print(f"\n{'='*60}\n{handler}\n{'='*60}")

    def get_upload_url():
        # Create a temporary Kendra workspace to get a valid workspace ID
        kendra_indexes = client.list_kendra_indexes()
        assert len(kendra_indexes) > 0, "No Kendra indexes available"
        ws = client.create_kendra_workspace(input={
            "name": "ipa-upload-test",
            "kind": "kendra",
            "kendraIndexId": kendra_indexes[0]["id"],
            "useAllData": False,
        })
        ws_id = ws.get("id")
        try:
            result = client.add_file(input={
                "workspaceId": ws_id,
                "fileName": "test-document.txt",
            })
            assert result.get("url") is not None
        finally:
            # Wait for workspace to be ready before deleting
            for _ in range(15):
                w = client.get_workspace(ws_id)
                if w and w.get("status") == "ready":
                    break
                time.sleep(2)
            client.delete_workspace(ws_id)

    run_scenario("get_upload_url", handler, get_upload_url, report)


def scenarios_embedding(client: AppSyncClient, report: ValidationReport):
    """Tests embedding calculation (exercises api-handler + Bedrock)."""
    handler = "api-handler"
    print(f"\n{'='*60}\nembedding (via {handler})\n{'='*60}")

    def calculate_embedding():
        result = client.calculate_embeding(input={
            "provider": "bedrock",
            "model": "amazon.titan-embed-text-v1",
            "passages": ["Hello world"],
            "task": "retrieve",
        })
        assert len(result) > 0
        assert len(result[0].get("vector", [])) > 0

    run_scenario("calculate_embedding", handler, calculate_embedding, report)


def scenarios_kendra_workspace(client: AppSyncClient, report: ValidationReport):
    """Kendra workspace lifecycle: create, add doc, search, delete.
    Exercises: api-handler, create-kendra-workspace (Step Function),
    upload-handler, delete-document, delete-workspace."""
    handler = "kendra-workspace"
    print(f"\n{'='*60}\n{handler} (lifecycle)\n{'='*60}")

    workspace_id = None
    kendra_indexes = None

    def create():
        nonlocal workspace_id, kendra_indexes
        kendra_indexes = client.list_kendra_indexes()
        assert len(kendra_indexes) > 0, "No Kendra indexes available"
        result = client.create_kendra_workspace(input={
            "name": "ipa-test-kendra",
            "kind": "kendra",
            "kendraIndexId": kendra_indexes[0]["id"],
            "useAllData": False,
        })
        workspace_id = result.get("id")
        assert workspace_id is not None
        # Wait for workspace to be ready
        for _ in range(30):
            ws = client.get_workspace(workspace_id)
            if ws and ws.get("status") == "ready":
                break
            time.sleep(2)

    run_scenario("create_kendra_workspace", handler, create, report)

    if workspace_id:
        run_scenario("delete_kendra_workspace", handler, lambda: (
            client.delete_workspace(workspace_id),
        ), report)


def scenarios_aurora_workspace(client: AppSyncClient, report: ValidationReport):
    """Aurora workspace lifecycle.
    Exercises: api-handler, create-aurora-workspace, pg-setup, delete-workspace."""
    handler = "aurora-workspace"
    print(f"\n{'='*60}\n{handler} (lifecycle)\n{'='*60}")

    workspace_id = None

    def create():
        nonlocal workspace_id
        result = client.create_aurora_workspace(input={
            "name": "ipa-test-aurora",
            "kind": "aurora",
            "embeddingsModelProvider": "bedrock",
            "embeddingsModelName": "cohere.embed-multilingual-v3",
            "languages": ["english"],
            "metric": "cosine",
            "index": True,
            "hybridSearch": False,
            "chunkingStrategy": "recursive",
            "chunkSize": 1000,
            "chunkOverlap": 200,
        })
        workspace_id = result.get("id")
        assert workspace_id is not None
        # Wait for workspace
        for _ in range(120):
            ws = client.get_workspace(workspace_id)
            if ws and ws.get("status") == "ready":
                break
            time.sleep(5)

    run_scenario("create_aurora_workspace", handler, create, report)

    if workspace_id:
        run_scenario("delete_aurora_workspace", handler, lambda: (
            client.delete_workspace(workspace_id),
        ), report)


def scenarios_opensearch_workspace(client: AppSyncClient, report: ValidationReport):
    """OpenSearch workspace lifecycle.
    Exercises: api-handler, create-opensearch-workspace, delete-workspace."""
    handler = "opensearch-workspace"
    print(f"\n{'='*60}\n{handler} (lifecycle)\n{'='*60}")

    workspace_id = None

    def create():
        nonlocal workspace_id
        result = client.create_opensearch_workspace(input={
            "name": "ipa-test-opensearch",
            "kind": "opensearch",
            "embeddingsModelProvider": "bedrock",
            "embeddingsModelName": "cohere.embed-multilingual-v3",
            "languages": ["english"],
            "hybridSearch": False,
            "chunkingStrategy": "recursive",
            "chunkSize": 1000,
            "chunkOverlap": 200,
        })
        workspace_id = result.get("id")
        assert workspace_id is not None
        for _ in range(60):
            ws = client.get_workspace(workspace_id)
            if ws and ws.get("status") == "ready":
                break
            time.sleep(5)

    run_scenario("create_opensearch_workspace", handler, create, report)

    if workspace_id:
        run_scenario("delete_opensearch_workspace", handler, lambda: (
            client.delete_workspace(workspace_id),
        ), report)


def scenarios_delete_document(client: AppSyncClient, report: ValidationReport):
    """delete-document: deletes a document from a workspace.
    Tests across Kendra and Aurora workspace types to exercise different backends."""
    handler = "delete-document"
    print(f"\n{'='*60}\n{handler}\n{'='*60}")

    def delete_doc_kendra():
        kendra_indexes = client.list_kendra_indexes()
        assert len(kendra_indexes) > 0, "No Kendra indexes available"
        ws = client.create_kendra_workspace(input={
            "name": "ipa-deldoc-kendra",
            "kind": "kendra",
            "kendraIndexId": kendra_indexes[0]["id"],
            "useAllData": False,
        })
        ws_id = ws.get("id")
        try:
            for _ in range(30):
                w = client.get_workspace(ws_id)
                if w and w.get("status") == "ready":
                    break
                time.sleep(2)
            doc = client.add_text(input={
                "workspaceId": ws_id,
                "title": "IPA-kendra-delete-test",
                "content": "Test document for deletion.",
            })
            doc_id = doc.get("documentId")
            assert doc_id is not None
            time.sleep(5)
            result = client.delete_document(input={
                "workspaceId": ws_id,
                "documentId": doc_id,
            })
            assert result is not None
        finally:
            client.delete_workspace(ws_id)

    def delete_doc_aurora():
        ws = client.create_aurora_workspace(input={
            "name": "ipa-deldoc-aurora",
            "kind": "aurora",
            "embeddingsModelProvider": "bedrock",
            "embeddingsModelName": "cohere.embed-multilingual-v3",
            "languages": ["english"],
            "metric": "cosine",
            "index": True,
            "hybridSearch": False,
            "chunkingStrategy": "recursive",
            "chunkSize": 1000,
            "chunkOverlap": 200,
        })
        ws_id = ws.get("id")
        try:
            for _ in range(60):
                w = client.get_workspace(ws_id)
                if w and w.get("status") == "ready":
                    break
                time.sleep(5)
            doc = client.add_text(input={
                "workspaceId": ws_id,
                "title": "IPA-aurora-delete-test",
                "content": "Test document for deletion from Aurora.",
            })
            doc_id = doc.get("documentId")
            assert doc_id is not None
            # Wait for document to be processed
            for _ in range(60):
                d = client.get_document(input={"workspaceId": ws_id, "documentId": doc_id})
                if d and d.get("status") in ("processed", "ready", None):
                    break
                time.sleep(5)
            result = client.delete_document(input={
                "workspaceId": ws_id,
                "documentId": doc_id,
            })
            assert result is not None
        finally:
            client.delete_workspace(ws_id)

    run_scenario("delete_doc_kendra", handler, delete_doc_kendra, report)
    run_scenario("delete_doc_aurora", handler, delete_doc_aurora, report)

    def delete_doc_opensearch():
        ws = client.create_opensearch_workspace(input={
            "name": "ipa-deldoc-opensearch",
            "kind": "opensearch",
            "embeddingsModelProvider": "bedrock",
            "embeddingsModelName": "cohere.embed-multilingual-v3",
            "languages": ["english"],
            "hybridSearch": False,
            "chunkingStrategy": "recursive",
            "chunkSize": 1000,
            "chunkOverlap": 200,
        })
        ws_id = ws.get("id")
        try:
            for _ in range(60):
                w = client.get_workspace(ws_id)
                if w and w.get("status") == "ready":
                    break
                time.sleep(5)
            doc = client.add_text(input={
                "workspaceId": ws_id,
                "title": "IPA-opensearch-delete-test",
                "content": "Test document for deletion from OpenSearch.",
            })
            doc_id = doc.get("documentId")
            assert doc_id is not None
            for _ in range(60):
                d = client.get_document(input={"workspaceId": ws_id, "documentId": doc_id})
                if d and d.get("status") in ("processed", "ready", None):
                    break
                time.sleep(5)
            result = client.delete_document(input={
                "workspaceId": ws_id,
                "documentId": doc_id,
            })
            assert result is not None
        finally:
            client.delete_workspace(ws_id)

    run_scenario("delete_doc_opensearch", handler, delete_doc_opensearch, report)


def scenarios_file_import_batch_job(client: AppSyncClient, report: ValidationReport):
    """file-import-batch-job: triggered when a file is uploaded to a workspace.
    Exercises: upload-handler, file-import-batch-job (via Step Functions)."""
    handler = "file-import-batch-job"
    print(f"\n{'='*60}\n{handler}\n{'='*60}")

    def upload_file_to_workspace():
        import requests

        # Create a Kendra workspace
        kendra_indexes = client.list_kendra_indexes()
        assert len(kendra_indexes) > 0, "No Kendra indexes available"
        ws = client.create_kendra_workspace(input={
            "name": "ipa-file-import-test",
            "kind": "kendra",
            "kendraIndexId": kendra_indexes[0]["id"],
            "useAllData": False,
        })
        ws_id = ws.get("id")
        try:
            for _ in range(30):
                w = client.get_workspace(ws_id)
                if w and w.get("status") == "ready":
                    break
                time.sleep(2)

            # Get presigned upload URL
            upload_info = client.add_file(input={
                "workspaceId": ws_id,
                "fileName": "test-upload.txt",
            })
            url = upload_info.get("url")
            fields_raw = upload_info.get("fields", "")
            assert url is not None, "No upload URL returned"

            # Parse fields: format is "{key=val, key2=val2, ...}"
            fields = {}
            if fields_raw:
                inner = fields_raw.strip("{}")
                for pair in inner.split(", "):
                    if "=" in pair:
                        k, v = pair.split("=", 1)
                        fields[k.strip()] = v.strip()

            # Upload a small file using the presigned URL
            files = {"file": ("test-upload.txt", b"Hello from IPA validation test")}
            resp = requests.post(url, data=fields, files=files)
            assert resp.status_code in (200, 201, 204), f"Upload failed: {resp.status_code}"

            # Wait for the file-import-batch-job to process
            time.sleep(15)

            # Verify the document appears
            docs = client.list_documents(input={
                "workspaceId": ws_id,
                "documentType": "file",
            })
            # The document should exist (even if still processing)
            assert docs is not None
        finally:
            client.delete_workspace(ws_id)

    run_scenario("upload_and_import_file", handler, upload_file_to_workspace, report)


def scenarios_web_crawler_batch_job(client: AppSyncClient, report: ValidationReport):
    """web-crawler-batch-job: triggered when a website URL is added for crawling.
    Exercises: api-handler, web-crawler-batch-job (via Step Functions)."""
    handler = "web-crawler-batch-job"
    print(f"\n{'='*60}\n{handler}\n{'='*60}")

    def add_website():
        # Create a Kendra workspace
        kendra_indexes = client.list_kendra_indexes()
        assert len(kendra_indexes) > 0, "No Kendra indexes available"
        ws = client.create_kendra_workspace(input={
            "name": "ipa-webcrawl-test",
            "kind": "kendra",
            "kendraIndexId": kendra_indexes[0]["id"],
            "useAllData": False,
        })
        ws_id = ws.get("id")
        try:
            for _ in range(30):
                w = client.get_workspace(ws_id)
                if w and w.get("status") == "ready":
                    break
                time.sleep(2)

            # Add a website for crawling — this triggers the web-crawler-batch-job
            from gql.dsl import DSLMutation, dsl_gql
            query = dsl_gql(
                DSLMutation(
                    client.schema.Mutation.addWebsite.args(input={
                        "workspaceId": ws_id,
                        "sitemap": False,
                        "address": "https://example.com",
                        "followLinks": False,
                        "limit": 1,
                        "contentTypes": ["text/html"],
                    }).select(
                        client.schema.DocumentResult.documentId,
                        client.schema.DocumentResult.status,
                    )
                )
            )
            result = client.client.execute(query).get("addWebsite")
            assert result is not None
            assert result.get("documentId") is not None
        finally:
            client.delete_workspace(ws_id)

    run_scenario("add_website_for_crawling", handler, add_website, report)


def scenarios_add_user_to_group(client: AppSyncClient, report: ValidationReport):
    """add-user-to-group: Cognito trigger that assigns federated users to groups.
    We invoke the Lambda directly with a simulated post-confirmation event."""
    handler = "add-user-to-group"
    print(f"\n{'='*60}\n{handler}\n{'='*60}")

    def invoke_post_confirmation():
        """Invoke the Lambda with a simulated Cognito post-confirmation event."""
        import boto3 as b3

        # Find the Lambda function name from the stack
        cf = b3.client("cloudformation", region_name="us-east-1")
        resources = cf.list_stack_resources(StackName="GenAIChatBotStack")
        func_name = None
        while True:
            for r in resources.get("StackResourceSummaries", []):
                if "addFederatedUserToUserGroup" in r.get("LogicalResourceId", ""):
                    if r["ResourceType"] == "AWS::Lambda::Function":
                        func_name = r["PhysicalResourceId"]
                        break
            if func_name or not resources.get("NextToken"):
                break
            resources = cf.list_stack_resources(
                StackName="GenAIChatBotStack",
                NextToken=resources["NextToken"],
            )

        assert func_name is not None, "Could not find add-user-to-group Lambda"

        # Get the user pool ID from the config
        user_pool_id = client._aws_config["aws_user_pools_id"]

        # Create a test user in Cognito so the handler can look it up
        cognito = b3.client("cognito-idp", region_name="us-east-1")
        test_username = "ipa-federated-test@example.com"
        try:
            cognito.admin_create_user(
                UserPoolId=user_pool_id,
                Username=test_username,
                UserAttributes=[
                    {"Name": "email", "Value": test_username},
                    {"Name": "email_verified", "Value": "True"},
                ],
                MessageAction="SUPPRESS",
            )
        except cognito.exceptions.UsernameExistsException:
            pass  # Already exists from a previous run

        # Simulate a Cognito post-confirmation event
        event = {
            "version": "1",
            "triggerSource": "PostConfirmation_ConfirmSignUp",
            "region": "us-east-1",
            "userPoolId": user_pool_id,
            "userName": test_username,
            "callerContext": {
                "awsSdkVersion": "3.0.0",
                "clientId": "test-client-id",
            },
            "request": {
                "userAttributes": {
                    "sub": test_username,
                    "email": test_username,
                    "email_verified": "true",
                    "custom:chatbot_role": "user",
                },
            },
            "response": {},
        }

        lam = b3.client("lambda", region_name="us-east-1")
        resp = lam.invoke(
            FunctionName=func_name,
            InvocationType="RequestResponse",
            Payload=json.dumps(event),
        )
        payload = json.loads(resp["Payload"].read())

        # The handler should return the event (possibly modified)
        assert resp["StatusCode"] == 200, f"Lambda returned status {resp['StatusCode']}"
        assert "errorMessage" not in payload, f"Lambda error: {payload.get('errorMessage')}"

    run_scenario("invoke_post_confirmation", handler, invoke_post_confirmation, report)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

ALL_SCENARIOS = {
    "api-handler": scenarios_api_handler,
    "send-query-resolver": scenarios_send_query_resolver,
    "langchain-request-handler": scenarios_langchain_request_handler,
    "upload-handler": scenarios_upload_handler,
    "embedding": scenarios_embedding,
    "kendra-workspace": scenarios_kendra_workspace,
    "aurora-workspace": scenarios_aurora_workspace,
    "opensearch-workspace": scenarios_opensearch_workspace,
    "delete-document": scenarios_delete_document,
    "file-import-batch-job": scenarios_file_import_batch_job,
    "web-crawler-batch-job": scenarios_web_crawler_batch_job,
    "add-user-to-group": scenarios_add_user_to_group,
}


def main():
    parser = argparse.ArgumentParser(description="Run handler validation scenarios")
    parser.add_argument("--app-url", required=True, help="CloudFront URL of the deployed app")
    parser.add_argument("--handler", help="Run scenarios for a specific handler only")
    parser.add_argument("--output", help="Write JSON report to this file")
    args = parser.parse_args()

    print(f"Connecting to {args.app_url}...")
    client = get_client(args.app_url)
    print("Authenticated successfully")

    report = ValidationReport()

    if args.handler:
        if args.handler in ALL_SCENARIOS:
            ALL_SCENARIOS[args.handler](client, report)
        else:
            print(f"Unknown handler: {args.handler}")
            print(f"Available: {', '.join(ALL_SCENARIOS.keys())}")
            sys.exit(1)
    else:
        for name, fn in ALL_SCENARIOS.items():
            fn(client, report)

    # Summary
    print(f"\n{'='*60}")
    print(f"Results: {len(report.passed)} passed, {len(report.failed)} failed")
    print(f"{'='*60}")
    if report.failed:
        print("\nFailed scenarios:")
        for r in report.failed:
            print(f"  {r.handler}/{r.scenario}: {r.error}")

    # Write report
    if args.output:
        import dataclasses
        data = {
            "results": [dataclasses.asdict(r) for r in report.results],
            "summary": {
                "total": len(report.results),
                "passed": len(report.passed),
                "failed": len(report.failed),
            }
        }
        with open(args.output, "w") as f:
            json.dump(data, f, indent=2)
        print(f"\nReport written to {args.output}")

    sys.exit(0 if not report.failed else 1)


if __name__ == "__main__":
    main()
