#!/usr/bin/env python3
"""
Policy-swap validation: for each handler, replace its Lambda's inline policy
with the generated policy, run the relevant scenarios, then restore.
"""

import argparse
import boto3
import json
import sys
import time
from pathlib import Path

# Paths are anchored to this script's directory so it can be run from anywhere.
HERE = Path(__file__).parent

# Add integtests clients
CDK_DIR = HERE / "genai-chatbot-cdk"
sys.path.insert(0, str(CDK_DIR / "integtests"))

HANDLERS_DIR = HERE / "genai-chatbot" / "handlers"
RESULTS_DIR = HERE / "realworld_results"

# Lambda function name -> handler name
LOGICAL_TO_HANDLER = {
    "ChatBotApiRestApiGraphQLApiHandler140797C1": "api-handler",
    "ChatBotApiRealtimeResolverslambdaresolver5D979DE8": "send-query-resolver",
    "LangchainInterfaceRequestHandler472B9584": "langchain-request-handler",
    "BedrockAgentsInterfaceRequestHandler236EF437": "bedrock-agents-handler",
    "RagEnginesDataImportUploadHandlerDB43C77C": "upload-handler",
    "RagEnginesAuroraPgVectorCreateAuroraWorkspaceCreateAuroraWorkspaceFunctionEC174778": "create-aurora-workspace",
    "RagEnginesAuroraPgVectorDatabaseSetupFunction3EAE1D9D": "pg-setup",
    "RagEnginesOpenSearchVectorCreateOpenSearchWorkspaceCreateOpenSearchWorkspaceFunction8E852192": "create-opensearch-workspace",
    "RagEnginesWorkspacesDeleteDocumentDeleteDocumentFunctionF13C7D85": "delete-document",
    "RagEnginesWorkspacesDeleteWorkspaceDeleteWorkspaceFunction8A41CF72": "delete-workspace",
    "AuthenticationaddFederatedUserToUserGroup791FC331": "add-user-to-group",
}


def discover_lambda_names(region, stack_name="GenAIChatBotStack"):
    """Discover Lambda function physical names from CloudFormation stack."""
    cfn = boto3.client("cloudformation", region_name=region)
    handler_to_lambda = {}
    paginator = cfn.get_paginator("list_stack_resources")
    for page in paginator.paginate(StackName=stack_name):
        for r in page["StackResourceSummaries"]:
            if r["ResourceType"] == "AWS::Lambda::Function":
                logical = r["LogicalResourceId"]
                if logical in LOGICAL_TO_HANDLER:
                    handler_to_lambda[LOGICAL_TO_HANDLER[logical]] = r["PhysicalResourceId"]
    return handler_to_lambda


HANDLER_TO_LAMBDA = {}  # populated at runtime

# Which scenarios to run when validating each handler.
# For each handler, we run all scenario groups that exercise it.
HANDLER_TO_SCENARIOS = {
    "api-handler": [
        "api-handler", "embedding",
        "kendra-workspace", "aurora-workspace", "opensearch-workspace",
        "upload-handler", "delete-document", "web-crawler-batch-job",
    ],
    "send-query-resolver": ["send-query-resolver"],
    "langchain-request-handler": ["langchain-request-handler"],
    "upload-handler": ["upload-handler"],
    "create-aurora-workspace": ["aurora-workspace"],
    "pg-setup": ["aurora-workspace"],
    "create-opensearch-workspace": ["opensearch-workspace"],
    "delete-document": ["delete-document"],
    "delete-workspace": ["kendra-workspace", "aurora-workspace", "opensearch-workspace"],
    "add-user-to-group": ["add-user-to-group"],
}

# Batch jobs use different role structure — logical IDs for discovery
BATCH_LOGICAL_IDS = {
    "file-import-batch-job": {
        "logical_id": "RagEnginesDataImportFileImportBatchJobFileImportJob25E94A14",
        "scenarios": ["file-import-batch-job"],
    },
    "web-crawler-batch-job": {
        "logical_id": "RagEnginesDataImportWebCrawlerBatchJobWebCrawlerJobDC22C303",
        "scenarios": ["web-crawler-batch-job"],
    },
}


def discover_batch_job_arns(region, stack_name="GenAIChatBotStack"):
    """Discover Batch job definition ARNs from CloudFormation stack."""
    cfn = boto3.client("cloudformation", region_name=region)
    result = {}
    paginator = cfn.get_paginator("list_stack_resources")
    for page in paginator.paginate(StackName=stack_name):
        for r in page["StackResourceSummaries"]:
            if r["ResourceType"] == "AWS::Batch::JobDefinition":
                for handler_name, info in BATCH_LOGICAL_IDS.items():
                    if r["LogicalResourceId"] == info["logical_id"]:
                        result[handler_name] = {
                            "job_def_arn": r["PhysicalResourceId"],
                            "scenarios": info["scenarios"],
                        }
    return result


def get_lambda_role(lambda_client, func_name):
    """Get the execution role name for a Lambda function."""
    config = lambda_client.get_function_configuration(FunctionName=func_name)
    role_arn = config["Role"]
    return role_arn.split("/")[-1]


def get_batch_job_role(batch_client, job_def_arn):
    """Get the job role name for a Batch job definition."""
    resp = batch_client.describe_job_definitions(jobDefinitions=[job_def_arn])
    role_arn = resp["jobDefinitions"][0]["containerProperties"]["jobRoleArn"]
    return role_arn.split("/")[-1]


def get_inline_policies(iam_client, role_name):
    """Get all inline policy names and documents for a role."""
    policy_names = iam_client.list_role_policies(RoleName=role_name)["PolicyNames"]
    policies = {}
    for name in policy_names:
        resp = iam_client.get_role_policy(RoleName=role_name, PolicyName=name)
        policies[name] = resp["PolicyDocument"]
    return policies


def get_attached_policy_arns(iam_client, role_name):
    """Get all attached managed policy ARNs for a role."""
    resp = iam_client.list_attached_role_policies(RoleName=role_name)
    return [p["PolicyArn"] for p in resp.get("AttachedPolicies", [])]


def force_cold_start(lambda_client, func_name):
    """Force a Lambda cold start by updating the description.

    This recycles all warm containers, clearing any cached credentials
    or SSM parameter values from previous invocations.
    """
    try:
        lambda_client.update_function_configuration(
            FunctionName=func_name,
            Description=f"Force cold start {int(time.time())}",
        )
        # Wait for the update to complete
        waiter = lambda_client.get_waiter("function_updated_v2")
        waiter.wait(FunctionName=func_name, WaiterConfig={"Delay": 5, "MaxAttempts": 24})
    except Exception as e:
        print(f"  WARNING: Failed to force cold start for {func_name}: {e}")


def replace_all_policies(iam_client, role_name, new_policy_docs, attached_arns):
    """Delete all inline policies, detach all managed policies, and put new inline policies.
    
    Returns True on success, False if the policy document is malformed.
    """
    # Delete existing inline policies
    existing = iam_client.list_role_policies(RoleName=role_name)["PolicyNames"]
    for name in existing:
        iam_client.delete_role_policy(RoleName=role_name, PolicyName=name)

    # Detach managed policies
    for arn in attached_arns:
        iam_client.detach_role_policy(RoleName=role_name, PolicyArn=arn)

    # Put new policies
    try:
        for i, doc in enumerate(new_policy_docs):
            name = f"generated-policy-{i}"
            iam_client.put_role_policy(
                RoleName=role_name,
                PolicyName=name,
                PolicyDocument=json.dumps(doc),
            )
        return True
    except iam_client.exceptions.MalformedPolicyDocumentException as e:
        print(f"  ❌ Malformed policy document: {e}")
        return False


def wait_for_policy_propagation(app_url, label="policy change", max_attempts=12, interval=10):
    """Wait until IAM policy changes have propagated by smoke-testing the api-handler.

    Calls listWorkspaces (a DynamoDB-only route that doesn't need SSM) repeatedly
    until it succeeds. This confirms the api-handler Lambda is operating under
    its current (expected) policy, not a stale cached one.
    """
    sys.path.insert(0, str(HERE))
    import handler_scenarios as hs

    client = hs.get_client(app_url)
    for attempt in range(1, max_attempts + 1):
        try:
            client.list_workspaces()
            print(f"  Policy propagation verified after {attempt * interval}s ({label})")
            return
        except Exception:
            if attempt < max_attempts:
                time.sleep(interval)
            else:
                print(f"  WARNING: Policy propagation not confirmed after {max_attempts * interval}s ({label})")


def restore_all_policies(iam_client, role_name, original_inline, original_attached_arns):
    """Restore original inline and managed policies."""
    # Delete IPA policies
    existing = iam_client.list_role_policies(RoleName=role_name)["PolicyNames"]
    for name in existing:
        iam_client.delete_role_policy(RoleName=role_name, PolicyName=name)

    # Restore inline
    for name, doc in original_inline.items():
        iam_client.put_role_policy(
            RoleName=role_name,
            PolicyName=name,
            PolicyDocument=json.dumps(doc),
        )

    # Re-attach managed
    for arn in original_attached_arns:
        iam_client.attach_role_policy(RoleName=role_name, PolicyArn=arn)


def load_generated_policies(handler_name, policy_dir, policy_suffix):
    """Load generated policies for a handler."""
    path = Path(policy_dir) / f"{handler_name}_{policy_suffix}_policy.json"
    if not path.exists():
        return None
    with open(path) as f:
        return json.load(f)


def discover_trial_policies(handler_name, policy_dir, policy_suffix):
    """Discover all trial policy files for a handler.
    
    Returns a list of (trial_idx, policies) tuples, sorted by trial index.
    If no trial files exist, returns a single entry with the representative policy.
    """
    policy_dir = Path(policy_dir)
    trials = []
    
    # Look for trial files: <handler>_<suffix>_policy_trial_01.json, etc.
    import glob
    pattern = str(policy_dir / f"{handler_name}_{policy_suffix}_policy_trial_*.json")
    trial_files = sorted(glob.glob(pattern))
    
    if trial_files:
        for tf in trial_files:
            # Extract trial index from filename
            fname = Path(tf).stem  # e.g. "api-handler_llm_policy_trial_03"
            trial_idx = int(fname.split("_trial_")[-1])
            with open(tf) as f:
                trials.append((trial_idx, json.load(f)))
    else:
        # No trial files — use the representative policy as a single trial
        policies = load_generated_policies(handler_name, str(policy_dir), policy_suffix)
        if policies:
            trials.append((1, policies))
    
    return trials


def run_scenarios(app_url, scenario_names):
    """Run specific scenarios and return results."""
    # Import here to avoid circular deps
    sys.path.insert(0, str(HERE))
    import handler_scenarios as hs

    client = hs.get_client(app_url)
    report = hs.ValidationReport()

    for name in scenario_names:
        if name in hs.ALL_SCENARIOS:
            hs.ALL_SCENARIOS[name](client, report)

    return report


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--app-url", required=True)
    parser.add_argument("--handler", help="Only validate this handler")
    parser.add_argument("--output", default=str(HERE / "realworld_results" / "swap_validation.json"))
    parser.add_argument("--policy-dir", default=str(HERE / "realworld_results"),
                        help="Directory containing <handler>_*_policy.json files")
    parser.add_argument("--policy-suffix", default="ipa",
                        help="Policy file suffix: 'ipa' or 'llm' (loads <handler>_<suffix>_policy.json)")
    parser.add_argument("--trials", action="store_true",
                        help="Validate all trial policies (not just the representative/median)")
    parser.add_argument("--region", default="us-east-1")
    args = parser.parse_args()

    iam = boto3.client("iam")
    lam = boto3.client("lambda", region_name=args.region)
    batch = boto3.client("batch", region_name=args.region)

    # Discover Lambda function names from CloudFormation
    global HANDLER_TO_LAMBDA
    print("Discovering Lambda functions from CloudFormation stack...")
    HANDLER_TO_LAMBDA = discover_lambda_names(args.region)
    print(f"Found {len(HANDLER_TO_LAMBDA)} handlers: {list(HANDLER_TO_LAMBDA.keys())}")

    # Discover Batch job definitions
    BATCH_HANDLERS = discover_batch_job_arns(args.region)
    print(f"Found {len(BATCH_HANDLERS)} batch jobs: {list(BATCH_HANDLERS.keys())}")

    # Force cold starts on all Lambda handlers to clear any cached
    # credentials or SSM parameter values from previous runs.
    print("Forcing cold starts on all Lambda handlers...")
    for func_name in HANDLER_TO_LAMBDA.values():
        if func_name:
            force_cold_start(lam, func_name)
    print("Cold starts complete.\n")

    results = []

    # Lambda handlers
    handlers_to_test = HANDLER_TO_SCENARIOS.keys()
    if args.handler:
        handlers_to_test = [args.handler] if args.handler in HANDLER_TO_SCENARIOS else []

    for handler_name in handlers_to_test:
        func_name = HANDLER_TO_LAMBDA.get(handler_name)
        if not func_name:
            print(f"SKIP {handler_name}: no Lambda mapping")
            continue

        # Discover trials or load single policy
        if args.trials:
            trials = discover_trial_policies(handler_name, args.policy_dir, args.policy_suffix)
        else:
            policies = load_generated_policies(handler_name, args.policy_dir, args.policy_suffix)
            trials = [(1, policies)] if policies else []

        if not trials:
            print(f"SKIP {handler_name}: no generated policy")
            continue

        scenarios = HANDLER_TO_SCENARIOS[handler_name]
        role_name = get_lambda_role(lam, func_name)

        for trial_idx, generated_policies in trials:
            trial_label = f" (trial {trial_idx})" if len(trials) > 1 else ""

            print(f"\n{'='*60}")
            print(f"SWAP: {handler_name}{trial_label} (role: {role_name})")
            print(f"Scenarios: {scenarios}")
            print(f"{'='*60}")

            # Save original policies
            original = get_inline_policies(iam, role_name)
            original_attached = get_attached_policy_arns(iam, role_name)
            print(f"  Original inline policies: {list(original.keys())}")
            print(f"  Original managed policies: {original_attached}")

            # Swap to generated policies (remove all existing, including managed)
            print(f"  Swapping to generated-only policy ({len(generated_policies)} docs)...")
            swap_ok = replace_all_policies(iam, role_name, generated_policies, original_attached)

            if not swap_ok:
                results.append({
                    "handler": handler_name,
                    "trial": trial_idx,
                    "scenario": "policy_swap",
                    "success": False,
                    "error": "Malformed policy document — could not apply generated policy",
                    "policy_type": args.policy_suffix,
                })
                print(f"  Restoring original policies...")
                restore_all_policies(iam, role_name, original, original_attached)
                force_cold_start(lam, func_name)
                wait_for_policy_propagation(args.app_url, label=f"restore {handler_name}")
                print(f"  Restored.")
                continue

            time.sleep(10)  # IAM propagation delay

            # Verify the swap: only generated policies should remain
            verify_inline = iam.list_role_policies(RoleName=role_name)["PolicyNames"]
            verify_attached = get_attached_policy_arns(iam, role_name)
            print(f"  After swap — inline: {verify_inline}, managed: {verify_attached}")
            assert len(verify_attached) == 0, f"Managed policies not detached: {verify_attached}"

        # Run scenarios
            try:
                report = run_scenarios(args.app_url, scenarios)
                for r in report.results:
                    r_dict = {
                        "handler": handler_name,
                        "trial": trial_idx,
                        "scenario": r.scenario,
                        "success": r.success,
                        "error": r.error,
                        "policy_type": args.policy_suffix,
                    }
                    results.append(r_dict)
                    status = "✅" if r.success else "❌"
                    print(f"  {status} {r.scenario}: {r.error if r.error else 'OK'}")
            except Exception as e:
                print(f"  ❌ Scenario execution failed: {e}")
                results.append({
                    "handler": handler_name,
                    "trial": trial_idx,
                    "scenario": "execution_error",
                    "success": False,
                    "error": str(e),
                    "policy_type": args.policy_suffix,
                })
            finally:
                # Always restore
                print(f"  Restoring original policies...")
                restore_all_policies(iam, role_name, original, original_attached)
                force_cold_start(lam, func_name)
                wait_for_policy_propagation(args.app_url, label=f"restore {handler_name}")
                print(f"  Restored.")

    # Batch job handlers
    batch_handlers = BATCH_HANDLERS.keys()
    if args.handler:
        batch_handlers = [args.handler] if args.handler in BATCH_HANDLERS else []

    for handler_name in batch_handlers:
        info = BATCH_HANDLERS[handler_name]

        if args.trials:
            trials = discover_trial_policies(handler_name, args.policy_dir, args.policy_suffix)
        else:
            policies = load_generated_policies(handler_name, args.policy_dir, args.policy_suffix)
            trials = [(1, policies)] if policies else []

        if not trials:
            print(f"SKIP {handler_name}: no generated policy")
            continue

        role_name = get_batch_job_role(batch, info["job_def_arn"])

        for trial_idx, generated_policies in trials:
            trial_label = f" (trial {trial_idx})" if len(trials) > 1 else ""

            print(f"\n{'='*60}")
            print(f"SWAP: {handler_name}{trial_label} (batch role: {role_name})")
            print(f"{'='*60}")

            original = get_inline_policies(iam, role_name)
            original_attached = get_attached_policy_arns(iam, role_name)
            print(f"  Original inline policies: {list(original.keys())}")
            print(f"  Original managed policies: {original_attached}")

            print(f"  Swapping to generated-only policy...")
            swap_ok = replace_all_policies(iam, role_name, generated_policies, original_attached)

            if not swap_ok:
                results.append({
                    "handler": handler_name,
                    "trial": trial_idx,
                    "scenario": "policy_swap",
                    "success": False,
                    "error": "Malformed policy document — could not apply generated policy",
                    "policy_type": args.policy_suffix,
                })
                print(f"  Restoring original policies...")
                restore_all_policies(iam, role_name, original, original_attached)
                wait_for_policy_propagation(args.app_url, label=f"restore {handler_name}")
                print(f"  Restored.")
                continue

            time.sleep(10)

            # Verify the swap
            verify_inline = iam.list_role_policies(RoleName=role_name)["PolicyNames"]
            verify_attached = get_attached_policy_arns(iam, role_name)
            print(f"  After swap — inline: {verify_inline}, managed: {verify_attached}")
            assert len(verify_attached) == 0, f"Managed policies not detached: {verify_attached}"

            try:
                report = run_scenarios(args.app_url, info["scenarios"])
                for r in report.results:
                    r_dict = {
                        "handler": handler_name,
                        "trial": trial_idx,
                        "scenario": r.scenario,
                        "success": r.success,
                        "error": r.error,
                        "policy_type": args.policy_suffix,
                    }
                    results.append(r_dict)
                    status = "✅" if r.success else "❌"
                    print(f"  {status} {r.scenario}: {r.error if r.error else 'OK'}")
            except Exception as e:
                print(f"  ❌ Scenario execution failed: {e}")
                results.append({
                    "handler": handler_name,
                    "trial": trial_idx,
                    "scenario": "execution_error",
                    "success": False,
                    "error": str(e),
                    "policy_type": args.policy_suffix,
                })
            finally:
                print(f"  Restoring original policies...")
                restore_all_policies(iam, role_name, original, original_attached)
                wait_for_policy_propagation(args.app_url, label=f"restore {handler_name}")
                print(f"  Restored.")

    # Summary
    passed = [r for r in results if r["success"]]
    failed = [r for r in results if not r["success"]]
    print(f"\n{'='*60}")
    print(f"SWAP VALIDATION RESULTS: {len(passed)} passed, {len(failed)} failed out of {len(results)}")
    print(f"{'='*60}")
    if failed:
        print("\nFailed:")
        for r in failed:
            print(f"  {r['handler']}/{r['scenario']}: {r['error']}")

    with open(args.output, "w") as f:
        json.dump({"results": results, "summary": {"passed": len(passed), "failed": len(failed), "total": len(results)}}, f, indent=2)
    print(f"\nReport: {args.output}")


if __name__ == "__main__":
    main()
