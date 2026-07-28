#!/usr/bin/env python3
"""
Extract the CDK-deployed IAM policies (ground truth) for each handler.

The real-world evaluation compares the policy that iam-policy-autopilot (or an
LLM) generates for a handler against the policy CDK actually deployed for that
handler's execution role. Those deployed policies are the "ground truth" and are
NOT checked into the repository (see .gitignore) — they are account- and
deployment-specific, and are only used locally to compare against the generated
policies. This script regenerates them from a live deployment of
`aws-genai-llm-chatbot`.

For each handler it:
  1. Discovers the handler's execution role from the deployed CloudFormation
     stack (Lambda function role, or Batch job-definition role).
  2. Reads the role's inline policies (iam:ListRolePolicies + iam:GetRolePolicy)
     and attached managed policies (iam:ListAttachedRolePolicies).
  3. Writes them under `genai-chatbot/handlers/<handler>/deployed_policy/` in the
     exact shape the `realworld-evaluator` binary expects:
       - inline_<i>.json         : {"RoleName","PolicyName","PolicyDocument"}
       - attached_policies.json  : [{"PolicyName","PolicyArn"}, ...]

Only the `Action` fields of these documents are used by the evaluator's
action-count comparison; resource ARNs are ignored.

Usage:
    # Deploy aws-genai-llm-chatbot first (see EVALUATION.md), then:
    python extract_deployed_policies.py --region us-east-1

    # Only some handlers:
    python extract_deployed_policies.py --handlers api-handler upload-handler

Requires the same AWS credentials/permissions used to deploy the stack
(read-only IAM + CloudFormation + Lambda + Batch describe permissions).
"""

import argparse
import json
import sys
from pathlib import Path

import boto3

# Reuse the handler -> logical-id discovery maps and IAM read helpers from the
# swap-validation orchestrator so both stay in sync.
from swap_validation import (
    discover_lambda_names,
    discover_batch_job_arns,
    get_lambda_role,
    get_batch_job_role,
    get_inline_policies,
    get_attached_policy_arns,
)

HANDLERS_DIR = Path(__file__).parent / "genai-chatbot" / "handlers"


def write_deployed_policy(handler, role_name, inline_policies, attached_arns):
    """Write inline_*.json + attached_policies.json for one handler."""
    out_dir = HANDLERS_DIR / handler / "deployed_policy"
    out_dir.mkdir(parents=True, exist_ok=True)

    # Remove any stale files from a previous extraction.
    for stale in out_dir.glob("inline_*.json"):
        stale.unlink()

    for i, (policy_name, document) in enumerate(sorted(inline_policies.items())):
        record = {
            "RoleName": role_name,
            "PolicyName": policy_name,
            "PolicyDocument": document,
        }
        (out_dir / f"inline_{i}.json").write_text(json.dumps(record, indent=4))

    attached = [
        {"PolicyName": arn.split("/")[-1], "PolicyArn": arn} for arn in attached_arns
    ]
    (out_dir / "attached_policies.json").write_text(json.dumps(attached, indent=4))

    print(
        f"  {handler}: {len(inline_policies)} inline, {len(attached_arns)} managed "
        f"-> {out_dir.relative_to(Path(__file__).parent)}"
    )


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--region", default="us-east-1", help="AWS region [us-east-1]")
    ap.add_argument(
        "--stack-name", default="GenAIChatBotStack", help="CloudFormation stack name"
    )
    ap.add_argument(
        "--handlers",
        nargs="*",
        help="Only extract these handlers (default: all discoverable)",
    )
    args = ap.parse_args()

    iam = boto3.client("iam", region_name=args.region)
    lambda_client = boto3.client("lambda", region_name=args.region)
    batch_client = boto3.client("batch", region_name=args.region)

    print(f"Discovering handler roles in stack {args.stack_name} ({args.region}) ...")
    handler_to_lambda = discover_lambda_names(args.region, args.stack_name)
    handler_to_batch = discover_batch_job_arns(args.region, args.stack_name)

    # Build handler -> role-name across Lambda and Batch handlers.
    handler_roles = {}
    for handler, func_name in handler_to_lambda.items():
        try:
            handler_roles[handler] = get_lambda_role(lambda_client, func_name)
        except Exception as e:  # noqa: BLE001
            print(f"  WARNING: could not resolve role for {handler}: {e}")
    for handler, info in handler_to_batch.items():
        try:
            handler_roles[handler] = get_batch_job_role(
                batch_client, info["job_def_arn"]
            )
        except Exception as e:  # noqa: BLE001
            print(f"  WARNING: could not resolve batch role for {handler}: {e}")

    if not handler_roles:
        print(
            "No handler roles discovered. Is the stack deployed and are the "
            "credentials correct?",
            file=sys.stderr,
        )
        return 1

    wanted = set(args.handlers) if args.handlers else None
    print("Extracting deployed policies ...")
    extracted = 0
    for handler, role_name in sorted(handler_roles.items()):
        if wanted and handler not in wanted:
            continue
        inline_policies = get_inline_policies(iam, role_name)
        attached_arns = get_attached_policy_arns(iam, role_name)
        write_deployed_policy(handler, role_name, inline_policies, attached_arns)
        extracted += 1

    print(f"Done. Extracted ground-truth policies for {extracted} handler(s).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
