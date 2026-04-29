#!/usr/bin/env bash
# scan_cdk_security.sh — Run Checkov security scanner on all CDK stacks
#
# Scans the synthesized CloudFormation templates in each run's cdk.out/ directory.
# Produces per-run reports and a combined summary.
#
# Prerequisites:
#   pip3 install --user checkov
#   Node.js + npm (for CDK synth)
#
# Usage:
#   cd integration-tests/projects
#   bash scan_cdk_security.sh                              # synth (if needed) + scan all runs
#   bash scan_cdk_security.sh run_001 run_003              # scan specific runs
#   bash scan_cdk_security.sh --json                       # output JSON reports
#   bash scan_cdk_security.sh --synth                      # force re-synth even if cdk.out exists
#   bash scan_cdk_security.sh --no-synth                   # skip synth, fail if no template
#   bash scan_cdk_security.sh --skip-check CKV_AWS_363     # skip additional check(s)
#   bash scan_cdk_security.sh --no-default-skips           # disable the default skip list
#
# Output:
#   Console summary + per-run results in scan_results/

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESULTS_DIR="${SCRIPT_DIR}/scan_results"
OUTPUT_FORMAT="cli"
USE_DEFAULT_SKIPS=true
SYNTH_MODE="auto"   # auto = synth only when cdk.out is missing; force = always; skip = never
RUNS=()

# ── Default skip list ────────────────────────────────────────────────────────
# These checks are suppressed by default because they are not relevant to the
# security posture of these example CDK stacks. Each suppression is documented
# with a rationale. To disable all default skips, pass --no-default-skips.
# To skip additional checks on top of these, use --skip-check.
#
# --- CDK framework internals (auto-generated Lambdas, not user code) --------
#
#   CKV_AWS_115 — Lambda concurrency limit: not needed for deploy-time-only
#                 Lambdas (e.g. CustomS3AutoDeleteObjects). These run once
#                 during stack create/update/delete, not in production.
#
#   CKV_AWS_116 — Lambda DLQ: not needed for synchronous CloudFormation
#                 custom resource handlers. CloudFormation retries on failure.
#
#   CKV_AWS_117 — Lambda VPC placement: not needed for Lambdas that only call
#                 public AWS APIs (S3). VPC would require NAT Gateway for no benefit.
#
# --- Project-level suppressions (example/demo infrastructure) ----------------
#
#   CKV_AWS_18  — S3 access logging: these are ephemeral example buckets, not
#                 production data stores. Access logging adds cost with no
#                 benefit for short-lived demo infrastructure.
#
#   CKV_AWS_21  — S3 versioning: same rationale as above; versioning is
#                 unnecessary for throwaway example buckets.
#
#   CKV_AWS_27  — SQS encryption: example queues carry no sensitive data.
#                 Default SSE-SQS encryption is sufficient.
#
#   CKV_AWS_28  — DynamoDB point-in-time recovery: example tables are
#                 ephemeral and contain no data worth recovering.
#
#   CKV_AWS_35  — CloudTrail KMS CMK encryption: the default SSE-S3 encryption
#                 on the trail bucket is acceptable for examples. CMK adds
#                 cost and key management overhead.
#
#   CKV_AWS_65  — ECS task definition host networking: not a concern for
#                 these example stacks that don't run production workloads.
#
#   CKV_AWS_119 — DynamoDB table encryption with CMK: default AWS-managed
#                 encryption is sufficient for example tables.
#
#   CKV_AWS_149 — Secrets Manager KMS CMK encryption: default AWS-managed
#                 encryption is sufficient for example secrets.
#
#   CKV_AWS_154 — Redshift outside VPC: for short-lived test clusters with
#                 publiclyAccessible=false and scoped IAM roles, VPC placement
#                 adds no meaningful security benefit. The cluster has no public
#                 endpoint and contains no real data.
#
#   CKV_AWS_158 — CloudWatch Log Group KMS encryption: default encryption
#                 is sufficient for example log groups.
#
#   CKV_AWS_64  — Redshift encryption at rest: example clusters contain no
#                 real data. Enabling encryption adds KMS key management
#                 overhead with no benefit for demo infrastructure.
#
#   CKV_AWS_71  — Redshift audit logging: not needed for ephemeral example
#                 clusters that are destroyed after testing.
#
DEFAULT_SKIP_CHECKS="CKV_AWS_115,CKV_AWS_116,CKV_AWS_117,CKV_AWS_18,CKV_AWS_21,CKV_AWS_27,CKV_AWS_28,CKV_AWS_35,CKV_AWS_64,CKV_AWS_65,CKV_AWS_71,CKV_AWS_119,CKV_AWS_149,CKV_AWS_154,CKV_AWS_158"

EXTRA_SKIP_CHECKS=""

# ── Parse arguments ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --json)                OUTPUT_FORMAT="json"; shift ;;
    --synth)               SYNTH_MODE="force"; shift ;;
    --no-synth)            SYNTH_MODE="skip"; shift ;;
    --no-default-skips)    USE_DEFAULT_SKIPS=false; shift ;;
    --skip-check)
      # Append user-specified checks to skip (comma-separated)
      if [[ -n "${2:-}" ]]; then
        EXTRA_SKIP_CHECKS="${EXTRA_SKIP_CHECKS:+${EXTRA_SKIP_CHECKS},}$2"
        shift 2
      else
        echo "Error: --skip-check requires a value (e.g. --skip-check CKV_AWS_363)"
        exit 1
      fi
      ;;
    --help|-h)
      echo "Usage: $0 [OPTIONS] [run_001 run_002 ...]"
      echo ""
      echo "Options:"
      echo "  --json                Output JSON reports (default: CLI)"
      echo "  --synth               Force CDK synth even if cdk.out/ already exists"
      echo "  --no-synth            Skip CDK synth; fail if no template found"
      echo "  --skip-check CKV_ID   Skip additional check(s), comma-separated"
      echo "                        (e.g. --skip-check CKV_AWS_363)"
      echo "  --no-default-skips    Disable the built-in default skip list"
      echo "  run_NNN               Scan only specified runs (default: all)"
      echo ""
      echo "By default (auto mode), CDK synth runs only when cdk.out/ is missing."
      echo ""
      echo "Default skipped checks (edit DEFAULT_SKIP_CHECKS in script to change):"
      echo "  CDK internals:"
      echo "    CKV_AWS_115  Lambda concurrency limit (CDK auto-generated Lambdas)"
      echo "    CKV_AWS_116  Lambda DLQ (CDK custom resource handlers)"
      echo "    CKV_AWS_117  Lambda VPC placement (CDK Lambdas calling public APIs)"
      echo "  Project-level:"
      echo "    CKV_AWS_18   S3 access logging"
      echo "    CKV_AWS_21   S3 versioning"
      echo "    CKV_AWS_27   SQS encryption with CMK"
      echo "    CKV_AWS_28   DynamoDB point-in-time recovery"
      echo "    CKV_AWS_35   CloudTrail KMS CMK encryption"
      echo "    CKV_AWS_64   Redshift encryption at rest"
      echo "    CKV_AWS_65   ECS task definition host networking"
      echo "    CKV_AWS_71   Redshift audit logging"
      echo "    CKV_AWS_119  DynamoDB CMK encryption"
      echo "    CKV_AWS_149  Secrets Manager CMK encryption"
      echo "    CKV_AWS_154  Redshift outside VPC (test clusters)"
      echo "    CKV_AWS_158  CloudWatch Log Group KMS encryption"
      exit 0
      ;;
    *)                     RUNS+=("$1"); shift ;;
  esac
done

# If no runs specified, discover all run_NNN directories (exclude run_results etc.)
if [[ ${#RUNS[@]} -eq 0 ]]; then
  for d in "${SCRIPT_DIR}"/run_[0-9]*/; do
    [[ -d "$d/cdk" ]] && RUNS+=("$(basename "$d")")
  done
fi

mkdir -p "$RESULTS_DIR"

# ── Build the combined skip list ─────────────────────────────────────────────
ALL_SKIPS=""
if [[ "$USE_DEFAULT_SKIPS" == "true" ]]; then
  ALL_SKIPS="${DEFAULT_SKIP_CHECKS}"
fi
if [[ -n "$EXTRA_SKIP_CHECKS" ]]; then
  ALL_SKIPS="${ALL_SKIPS:+${ALL_SKIPS},}${EXTRA_SKIP_CHECKS}"
fi
SKIP_FLAG=""
if [[ -n "$ALL_SKIPS" ]]; then
  SKIP_FLAG="--skip-check ${ALL_SKIPS}"
fi

# ── Summary counters ─────────────────────────────────────────────────────────
TOTAL_PASSED=0
TOTAL_FAILED=0
TOTAL_SKIPPED=0
SCANNED=0
ERRORS=()

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║          CDK Stack Security Scanner (Checkov)                ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""
echo "Runs to scan:          ${RUNS[*]}"
echo "Output format:         ${OUTPUT_FORMAT}"
echo "CDK synth mode:        ${SYNTH_MODE}"
echo "Default skips:         ${USE_DEFAULT_SKIPS}"
if [[ -n "$ALL_SKIPS" ]]; then
  echo "Skipped checks:        ${ALL_SKIPS}"
fi
echo "Results dir:           ${RESULTS_DIR}"
echo ""

# ── Helper: synthesize a CDK stack ───────────────────────────────────────────
synth_cdk() {
  local cdk_dir="$1"
  echo "  📦 Installing CDK dependencies..."
  (cd "$cdk_dir" && npm install --no-audit --no-fund 2>&1 | tail -1)
  echo "  🔨 Synthesizing CloudFormation template..."
  (cd "$cdk_dir" && npx cdk synth --quiet 2>&1)
}

# ── Scan each run ────────────────────────────────────────────────────────────
for RUN in "${RUNS[@]}"; do
  RUN_DIR="${SCRIPT_DIR}/${RUN}"
  CDK_DIR="${RUN_DIR}/cdk"
  CDK_OUT="${CDK_DIR}/cdk.out"

  echo "────────────────────────────────────────────────────────────────"
  echo "▶ Scanning ${RUN}..."

  # ── CDK synth (auto/force/skip) ──────────────────────────────────────────
  if [[ "$SYNTH_MODE" == "force" ]]; then
    synth_cdk "$CDK_DIR"
  elif [[ "$SYNTH_MODE" == "auto" ]]; then
    # Only synth when cdk.out is missing or contains no templates
    if ! find "$CDK_OUT" -maxdepth 1 -name "*.template.json" 2>/dev/null | grep -q .; then
      echo "  ℹ No cdk.out found — running CDK synth automatically..."
      synth_cdk "$CDK_DIR"
    fi
  fi
  # SYNTH_MODE == "skip" → do nothing, rely on existing cdk.out

  # Find the template file (skip node_modules artifacts)
  TEMPLATE=$(find "$CDK_OUT" -maxdepth 1 -name "*.template.json" 2>/dev/null | head -1)

  if [[ -z "$TEMPLATE" ]]; then
    echo "  ⚠ No synthesized template found in ${CDK_OUT}"
    echo "  → Run 'cd ${CDK_DIR} && npm install && npx cdk synth' first"
    ERRORS+=("${RUN}: no template")
    continue
  fi

  TEMPLATE_NAME="$(basename "$TEMPLATE")"
  echo "  Template: ${TEMPLATE_NAME}"

  # Determine output file
  if [[ "$OUTPUT_FORMAT" == "json" ]]; then
    OUT_FILE="${RESULTS_DIR}/${RUN}_checkov.json"
    FORMAT_FLAG="--output json"
  else
    OUT_FILE="${RESULTS_DIR}/${RUN}_checkov.txt"
    FORMAT_FLAG="--output cli"
  fi

  # Run checkov
  set +e
  checkov \
    --file "$TEMPLATE" \
    --framework cloudformation \
    --compact \
    --quiet \
    ${FORMAT_FLAG} \
    ${SKIP_FLAG} \
    2>&1 | tee "$OUT_FILE"
  EXIT_CODE=$?
  set -e

  # Extract counts from the output
  if [[ "$OUTPUT_FORMAT" == "cli" ]]; then
    PASSED=$(grep -oP 'Passed checks: \K\d+' "$OUT_FILE" 2>/dev/null || echo "0")
    FAILED=$(grep -oP 'Failed checks: \K\d+' "$OUT_FILE" 2>/dev/null || echo "0")
    SKIPPED=$(grep -oP 'Skipped checks: \K\d+' "$OUT_FILE" 2>/dev/null || echo "0")
  else
    PASSED=$(python3 -c "
import json, sys
try:
    data = json.load(open('$OUT_FILE'))
    print(data.get('summary', {}).get('passed', 0))
except: print(0)
" 2>/dev/null || echo "0")
    FAILED=$(python3 -c "
import json, sys
try:
    data = json.load(open('$OUT_FILE'))
    print(data.get('summary', {}).get('failed', 0))
except: print(0)
" 2>/dev/null || echo "0")
    SKIPPED=0
  fi

  TOTAL_PASSED=$((TOTAL_PASSED + PASSED))
  TOTAL_FAILED=$((TOTAL_FAILED + FAILED))
  TOTAL_SKIPPED=$((TOTAL_SKIPPED + SKIPPED))
  SCANNED=$((SCANNED + 1))

  echo "  Results: ✅ ${PASSED} passed | ❌ ${FAILED} failed | ⏭ ${SKIPPED} skipped"
  echo "  Report:  ${OUT_FILE}"
  echo ""
done

# ── Summary ──────────────────────────────────────────────────────────────────
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║                      SCAN SUMMARY                            ║"
echo "╠══════════════════════════════════════════════════════════════╣"
printf "║  Stacks scanned : %-40s║\n" "$SCANNED"
printf "║  Total passed   : %-40s║\n" "✅ $TOTAL_PASSED"
printf "║  Total failed   : %-40s║\n" "❌ $TOTAL_FAILED"
printf "║  Total skipped  : %-40s║\n" "⏭  $TOTAL_SKIPPED"
if [[ ${#ERRORS[@]} -gt 0 ]]; then
  printf "║  Errors         : %-40s║\n" "⚠ ${#ERRORS[@]}"
  for err in "${ERRORS[@]}"; do
    printf "║    - %-54s║\n" "$err"
  done
fi
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""
echo "Detailed reports saved to: ${RESULTS_DIR}/"

if [[ $TOTAL_FAILED -gt 0 ]]; then
  exit 1
fi
