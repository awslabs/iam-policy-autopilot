#!/usr/bin/env python3
"""
Aggregate real-world evaluation results into a summary table.

Reads the evaluation reports and swap validation files from the results/
directory and produces a per-handler summary matching the paper's table.

The action counts in each evaluation_report.json are *concrete* IAM actions
after wildcard expansion via the service catalogue (e.g., `bedrock-agentcore:*`
expands to 148 actions). These are computed during policy generation.

For LLM experiments, the evaluation_report.json stores the concrete action
count of the *median trial* (selected by sorting trials by concrete action
count and picking the middle one).

Columns:
  Handler | IPA | Auth. | LLM_b | LLM_w | IPA_val | LLM_b_val | LLM_w_val

Where:
  IPA       = concrete IAM actions in the IPA-generated policy
  Auth.     = concrete IAM actions in the authored (CDK-deployed) policy
  LLM_b     = concrete IAM actions in the LLM-bare median trial policy
  LLM_w     = concrete IAM actions in the LLM-wildcards median trial policy
  IPA_val   = IPA live validation pass/fail
  LLM_b_val = LLM-bare trials passing (out of 5)
  LLM_w_val = LLM-wildcards trials passing (out of 5)

Aggregate factor row: sum of each column / sum of IPA column.

Usage:
    python aggregate_results.py [--results-dir results/]
"""

import argparse
import json
from collections import defaultdict
from pathlib import Path


def load_json(path: Path) -> dict:
    with open(path) as f:
        return json.load(f)


def build_eval_lookup(report: dict) -> dict:
    """Build handler -> eval entry lookup from an evaluation_report.json."""
    return {h["handler"]: h for h in report["handlers"]}


def compute_ipa_validation(swap_file: Path) -> dict:
    """
    Compute per-handler IPA validation result.

    IPA is deterministic (single run). A handler passes if ALL its scenarios
    in the swap validation succeeded.

    Returns: {handler: True/False}
    """
    data = load_json(swap_file)
    handler_results = defaultdict(list)
    for r in data["results"]:
        handler_results[r["handler"]].append(r["success"])
    return {h: all(results) for h, results in handler_results.items()}


def compute_llm_trial_validation(swap_file: Path) -> dict:
    """
    Compute per-handler LLM trial validation counts.

    A trial passes if ALL its scenarios succeeded. A "policy_swap" scenario
    with "Malformed policy document" means the trial failed entirely.

    Returns: {handler: number_of_passing_trials}
    """
    data = load_json(swap_file)

    # Group by (handler, trial)
    trial_results = defaultdict(lambda: defaultdict(list))
    for r in data["results"]:
        trial_results[r["handler"]][r["trial"]].append(r["success"])

    handler_pass_counts = {}
    for handler, trials in trial_results.items():
        passing = sum(1 for trial_successes in trials.values()
                      if all(trial_successes))
        handler_pass_counts[handler] = passing

    return handler_pass_counts


# Short display names matching the paper table
DISPLAY_NAMES = {
    "create-aurora-workspace": "create-aurora-ws",
    "create-opensearch-workspace": "create-opensearch-ws",
    "file-import-batch-job": "file-import-batch",
    "langchain-request-handler": "langchain-req-handler",
    "web-crawler-batch-job": "web-crawler-batch",
}

# Handlers in paper table order (excluding bedrock-agents-handler which
# was not live-validated)
EVALUATED_HANDLERS = [
    "add-user-to-group",
    "api-handler",
    "create-aurora-workspace",
    "create-opensearch-workspace",
    "delete-document",
    "delete-workspace",
    "file-import-batch-job",
    "langchain-request-handler",
    "pg-setup",
    "send-query-resolver",
    "upload-handler",
    "web-crawler-batch-job",
]


def main():
    parser = argparse.ArgumentParser(description="Aggregate real-world evaluation results")
    parser.add_argument("--results-dir", type=Path, default=Path("results"),
                        help="Path to the results/ directory")
    args = parser.parse_args()

    results_dir = args.results_dir

    # --- Load evaluation reports ---
    # Each report's `ipa_concrete_action_count` is the expanded concrete action
    # count (after wildcard expansion via service catalogue). For LLM reports,
    # this is the count for the median trial.
    ipa_lookup = build_eval_lookup(
        load_json(results_dir / "ipa" / "evaluation_report.json"))
    bare_lookup = build_eval_lookup(
        load_json(results_dir / "llm-bare" / "evaluation_report.json"))
    wildcards_lookup = build_eval_lookup(
        load_json(results_dir / "llm-wildcards" / "evaluation_report.json"))

    # --- Load live validation results ---
    ipa_validation = compute_ipa_validation(
        results_dir / "ipa" / "swap_validation.json")
    bare_trial_val = compute_llm_trial_validation(
        results_dir / "llm-bare" / "swap_validation_trials.json")
    wildcards_trial_val = compute_llm_trial_validation(
        results_dir / "llm-wildcards" / "swap_validation_trials.json")

    # --- Collect per-handler data ---
    rows = []
    for handler in EVALUATED_HANDLERS:
        ipa_entry = ipa_lookup[handler]
        bare_entry = bare_lookup[handler]
        wildcards_entry = wildcards_lookup[handler]

        ipa_actions = ipa_entry["ipa_concrete_action_count"]
        auth_actions = ipa_entry["cdk_concrete_action_count"]
        llm_b_actions = bare_entry["ipa_concrete_action_count"]
        llm_w_actions = wildcards_entry["ipa_concrete_action_count"]

        ipa_val = ipa_validation.get(handler, False)
        llm_b_val = bare_trial_val.get(handler, 0)
        llm_w_val = wildcards_trial_val.get(handler, 0)

        rows.append({
            "handler": handler,
            "ipa": ipa_actions,
            "auth": auth_actions,
            "llm_b": llm_b_actions,
            "llm_w": llm_w_actions,
            "ipa_val": ipa_val,
            "llm_b_val": llm_b_val,
            "llm_w_val": llm_w_val,
        })

    # --- Print table ---
    col_w = 30
    hdr = (f"{'Handler':<{col_w}s} {'IPA':>5s} {'Auth.':>5s} {'LLM_b':>5s} {'LLM_w':>5s}"
           f"  {'IPA':>5s} {'LLM_b':>5s} {'LLM_w':>5s}")
    sep = "-" * len(hdr)
    print(hdr)
    print(sep)

    sum_ipa = sum_auth = sum_b = sum_w = 0
    for r in rows:
        display = DISPLAY_NAMES.get(r["handler"], r["handler"])
        ipa_sym = "✓" if r["ipa_val"] else "✗"
        print(f"{display:<{col_w}s} {r['ipa']:>5d} {r['auth']:>5d} "
              f"{r['llm_b']:>5d} {r['llm_w']:>5d}  {ipa_sym:>5s} "
              f"{r['llm_b_val']:>3d}/5 {r['llm_w_val']:>3d}/5")
        sum_ipa += r["ipa"]
        sum_auth += r["auth"]
        sum_b += r["llm_b"]
        sum_w += r["llm_w"]

    print(sep)
    auth_f = sum_auth / sum_ipa if sum_ipa else 0
    b_f = sum_b / sum_ipa if sum_ipa else 0
    w_f = sum_w / sum_ipa if sum_ipa else 0
    print(f"{'Agg. factor (vs. IPA)':<{col_w}s} {'1.0x':>5s} {auth_f:>4.1f}x "
          f"{b_f:>4.1f}x {w_f:>4.1f}x")

    # --- Write JSON ---
    output = {
        "handlers": rows,
        "aggregate": {
            "sum_ipa": sum_ipa,
            "sum_authored": sum_auth,
            "sum_llm_bare": sum_b,
            "sum_llm_wildcards": sum_w,
            "factor_authored_vs_ipa": round(auth_f, 1),
            "factor_llm_bare_vs_ipa": round(b_f, 1),
            "factor_llm_wildcards_vs_ipa": round(w_f, 1),
        },
    }
    json_path = results_dir / "aggregate_table.json"
    with open(json_path, "w") as f:
        json.dump(output, f, indent=2)
    print(f"\nJSON written to {json_path}")


if __name__ == "__main__":
    main()
