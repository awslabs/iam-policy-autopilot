#!/usr/bin/env bash
#
# Run the synthetic IaC benchmark over the in-repo test projects
# (integration-tests/projects/run_001 .. run_010) and write an
# aggregate_report.json plus per-run logs.
#
# This deploys CDK stacks, creates temporary IAM roles, runs application scripts
# against live AWS APIs, and (optionally) calls Bedrock and iamfast. It incurs
# AWS charges and must be run against a NON-PRODUCTION account.
#
# Prerequisites: build.sh has been run; AWS credentials are exported; Node/CDK
# and the per-language toolchains (python3, go, java+maven, node/ts-node) are
# installed.
#
# Environment overrides:
#   RUNS_DIR              input runs directory    [default: integration-tests/projects]
#   OUTPUT_DIR            results output directory [default: paper-benchmarks/results]
#                         (aggregate_report.json is written directly here)
#   AUTOPILOT_BIN         IPA binary               [default: target/release/iam-policy-autopilot]
#   AWS_DEFAULT_REGION    AWS region               [default: us-east-1]
#   BEDROCK_MODEL_ID      Bedrock model / inference-profile ARN for the LLM phase
#                         [default: tool picks an inference profile from region + account]
#   CONTEXT_SCENARIOS_DIR scenario files for the context-filling LLM experiment
#                         [default: iac-benchmarker/scenarios]
#   IAMFAST_BIN           iamfast CLI for the static-analysis comparison
#                         [default: unset -> pass --skip-iamfast unless you set it]
#
# Any extra arguments are forwarded to iac-benchmarker, e.g.:
#   ./run_iac_benchmark.sh --skip-llm --skip-iamfast
#   ./run_iac_benchmark.sh --skip-validation
#   ./run_iac_benchmark.sh --skip run_003 run_007
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH_DIR="$(dirname "$SCRIPT_DIR")"
REPO_ROOT="$(dirname "$BENCH_DIR")"

RUNS_DIR="${RUNS_DIR:-$REPO_ROOT/integration-tests/projects}"
AUTOPILOT_BIN="${AUTOPILOT_BIN:-$REPO_ROOT/target/release/iam-policy-autopilot}"
BENCHMARKER="$BENCH_DIR/target/release/iac-benchmarker"
OUTPUT_DIR="${OUTPUT_DIR:-$BENCH_DIR/results}"
CONTEXT_SCENARIOS_DIR="${CONTEXT_SCENARIOS_DIR:-$BENCH_DIR/iac-benchmarker/scenarios}"

for f in "$BENCHMARKER" "$AUTOPILOT_BIN"; do
  if [[ ! -x "$f" ]]; then
    echo "error: $f not found. Run scripts/build.sh first." >&2
    exit 1
  fi
done

# Assemble optional flags from environment overrides.
extra_args=()
if [[ -n "${BEDROCK_MODEL_ID:-}" ]]; then
  extra_args+=(--bedrock-model-id "$BEDROCK_MODEL_ID")
fi
if [[ -n "$CONTEXT_SCENARIOS_DIR" && -d "$CONTEXT_SCENARIOS_DIR" ]]; then
  extra_args+=(--context-scenarios-dir "$CONTEXT_SCENARIOS_DIR")
fi
if [[ -n "${IAMFAST_BIN:-}" ]]; then
  extra_args+=(--iamfast-binary "$IAMFAST_BIN")
else
  # No iamfast CLI provided; skip that comparison so the run doesn't fail.
  extra_args+=(--skip-iamfast)
fi

echo "==> Running IaC benchmark"
echo "    runs-dir:        $RUNS_DIR"
echo "    autopilot-binary:$AUTOPILOT_BIN"
echo "    output-dir:      $OUTPUT_DIR"

"$BENCHMARKER" \
  --runs-dir "$RUNS_DIR" \
  --autopilot-binary "$AUTOPILOT_BIN" \
  --output-dir "$OUTPUT_DIR" \
  "${extra_args[@]}" \
  "$@"

echo
echo "Done. Wrote $OUTPUT_DIR/aggregate_report.json plus per-run logs."
echo "Generate figures with: scripts/make_figures.sh $OUTPUT_DIR/aggregate_report.json"
