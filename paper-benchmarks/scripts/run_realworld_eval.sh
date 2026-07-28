#!/usr/bin/env bash
#
# Run the real-world benchmark on aws-genai-llm-chatbot.
#
# The FULL evaluation has three phases per mode:
#
#   1. DEPLOY   — deploy the chatbot app to AWS (creates the handler roles and
#                 the live GraphQL/CloudFront endpoint the swap test drives).
#   2. EVALUATE — for each handler, generate a policy and compare it against
#                 the CDK-deployed ground truth (writes the evaluation report).
#   3. VALIDATE — live policy-swap validation: replace each handler's role policy
#                 with the generated one, run its scenarios through the live
#                 app, then restore.
#
# Phase 1 (deploy) incurs costs — the app uses Aurora, OpenSearch Serverless, and
# Kendra — and can need manual build fixes, so this script does NOT deploy for
# you. Deploy once (commands below), then re-run this script with --app-url to do
# phases 2+3 against the running stack. Tear the stack down when finished.
#
# ── Phase 1: deploy the app to us-east-1 (run these once, by hand) ──────────
#   export AWS_DEFAULT_REGION=us-east-1
#   ./paper-benchmarks/scripts/prepare_realworld_app.sh          # extract + config overlay
#   cd paper-benchmarks/real-world-apps/genai-chatbot-cdk
#   npm install && npm run build        # see EVALUATION.md for build fixes
#   npx cdk bootstrap                   # once per account/region
#   npx cdk deploy                      # note the CloudFront URL in the outputs
#   cd -
#
# ── Phases 2+3: evaluate and live-validate against the deployed stack ────────
#   ./paper-benchmarks/scripts/run_realworld_eval.sh --app-url https://<dist>.cloudfront.net
#
# Omit --app-url to run phase 2 only (static comparison against the committed
# ground truth — no deployment required, reproduces the CDK-vs-IPA action counts
# but not the live pass/fail results).
#
# Prerequisites: build.sh has been run; AWS credentials exported. For the deploy
# and validate phases you also need Node/CDK, Docker, and Bedrock model access.
#
# Environment overrides:
#   MODE            "all" (default), "ipa", "llm-bare", or "llm-wildcards"
#                   "all" runs ipa + llm-bare + llm-wildcards + aggregation.
#   AUTOPILOT_BIN   IPA binary        [default: target/release/iam-policy-autopilot]
#   RESULTS_DIR     top-level results  [default: real-world-apps/realworld_results]
#   LLM_TRIALS      number of LLM trials per handler [default: 5]
#
# Extra arguments are forwarded to realworld-evaluator, e.g.:
#   ./run_realworld_eval.sh --app-url https://... --only api-handler,upload-handler
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH_DIR="$(dirname "$SCRIPT_DIR")"
REPO_ROOT="$(dirname "$BENCH_DIR")"

RWA_DIR="$BENCH_DIR/real-world-apps"
EVALUATOR="$BENCH_DIR/target/release/realworld-evaluator"
AUTOPILOT_BIN="${AUTOPILOT_BIN:-$REPO_ROOT/target/release/iam-policy-autopilot}"
MODE="${MODE:-all}"
RESULTS_DIR="${RESULTS_DIR:-$RWA_DIR/realworld_results}"
LLM_TRIALS="${LLM_TRIALS:-5}"

# Pull --app-url out of the args; everything else is forwarded to the evaluator.
APP_URL=""
EVAL_ARGS=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --app-url) APP_URL="$2"; shift 2 ;;
    --app-url=*) APP_URL="${1#*=}"; shift ;;
    *) EVAL_ARGS+=("$1"); shift ;;
  esac
done

for f in "$EVALUATOR" "$AUTOPILOT_BIN"; do
  if [[ ! -x "$f" ]]; then
    echo "error: $f not found. Run scripts/build.sh first." >&2
    exit 1
  fi
done

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

ensure_cognito_auth() {
  echo "==> Ensuring Cognito client allows ADMIN_USER_PASSWORD_AUTH for test scripts"
  local pool_id client_id
  pool_id=$(aws cloudformation describe-stacks --stack-name GenAIChatBotStack --region us-east-1 \
    --query "Stacks[0].Outputs[?OutputKey=='AuthenticationUserPoolIdF0D106F7'].OutputValue" --output text)
  client_id=$(aws cloudformation describe-stacks --stack-name GenAIChatBotStack --region us-east-1 \
    --query "Stacks[0].Outputs[?OutputKey=='AuthenticationUserPoolWebClientId80D5526A'].OutputValue" --output text)
  aws cognito-idp update-user-pool-client \
    --region us-east-1 \
    --user-pool-id "$pool_id" \
    --client-id "$client_id" \
    --explicit-auth-flows ALLOW_ADMIN_USER_PASSWORD_AUTH ALLOW_USER_SRP_AUTH ALLOW_REFRESH_TOKEN_AUTH \
    > /dev/null
}

# run_mode <mode>
# Runs phases 2+3 for a single mode. Writes into $RESULTS_DIR/<subdir>/.
run_mode() {
  local mode="$1"
  local eval_mode resource_strategy output_subdir policy_suffix swap_output swap_trials_flag

  case "$mode" in
    ipa)
      eval_mode="ipa"
      resource_strategy=""
      output_subdir="ipa"
      policy_suffix="ipa"
      swap_output="swap_validation.json"
      swap_trials_flag=""
      ;;
    llm-bare)
      eval_mode="llm"
      resource_strategy="bare"
      output_subdir="llm-bare"
      policy_suffix="llm"
      swap_output="swap_validation_trials.json"
      swap_trials_flag="--trials"
      ;;
    llm-wildcards)
      eval_mode="llm"
      resource_strategy="wildcards"
      output_subdir="llm-wildcards"
      policy_suffix="llm"
      swap_output="swap_validation_trials.json"
      swap_trials_flag="--trials"
      ;;
    *)
      echo "error: unknown mode '$mode'" >&2; exit 1 ;;
  esac

  local output_dir="$RESULTS_DIR/$output_subdir"
  mkdir -p "$output_dir"

  echo
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo "  MODE: $mode → $output_dir"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

  # Build evaluator arguments
  local -a evaluator_args=(
    --handlers-dir "$RWA_DIR/genai-chatbot/handlers"
    --shared-layer-dir "$RWA_DIR/genai-chatbot/shared_layer/genai_core"
    --autopilot-binary "$AUTOPILOT_BIN"
    --region us-east-1
    --cache-dir "$RWA_DIR/.cache"
    --output-dir "$output_dir"
    --mode "$eval_mode"
  )
  if [[ -n "$resource_strategy" ]]; then
    evaluator_args+=(--resource-strategy "$resource_strategy")
  fi
  if [[ "$eval_mode" == "llm" ]]; then
    evaluator_args+=(--llm-repetitions "$LLM_TRIALS")
  fi

  # Phase 2: evaluate
  echo "==> Phase 2: evaluate ($mode)"
  "$EVALUATOR" "${evaluator_args[@]}" "${EVAL_ARGS[@]}"

  # Phase 3: live policy-swap validation
  if [[ -n "$APP_URL" ]]; then
    echo "==> Phase 3: live policy-swap validation ($mode)"
    python3 "$RWA_DIR/swap_validation.py" \
      --app-url "$APP_URL" \
      --region us-east-1 \
      --policy-dir "$output_dir" \
      --policy-suffix "$policy_suffix" \
      --output "$output_dir/$swap_output" \
      $swap_trials_flag
  fi
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

cat <<BANNER
==> Real-world benchmark
    Phase 1 DEPLOY   : deploy the app yourself (see the header of this script / EVALUATION.md)
    Phase 2 EVALUATE : generate policies + compare against CDK ground truth
    Phase 3 VALIDATE : live policy-swap test $( [[ -n "$APP_URL" ]] && echo "[enabled, --app-url set]" || echo "[SKIPPED — pass --app-url to enable]" )

    mode:         $MODE
    results-dir:  $RESULTS_DIR
    region:       us-east-1
BANNER

# Ensure Cognito auth flow is configured (once, before all modes)
if [[ -n "$APP_URL" ]]; then
  ensure_cognito_auth
fi

# Determine which modes to run
if [[ "$MODE" == "all" ]]; then
  MODES=(ipa llm-bare llm-wildcards)
else
  MODES=("$MODE")
fi

for m in "${MODES[@]}"; do
  run_mode "$m"
done

# Aggregation (only if all three modes have results)
if [[ "$MODE" == "all" ]] || {
  [[ -f "$RESULTS_DIR/ipa/evaluation_report.json" ]] &&
  [[ -f "$RESULTS_DIR/llm-bare/evaluation_report.json" ]] &&
  [[ -f "$RESULTS_DIR/llm-wildcards/evaluation_report.json" ]]; }; then
  echo
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo "  Aggregating results"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  python3 "$RWA_DIR/aggregate_results.py" --results-dir "$RESULTS_DIR"
fi

echo
echo "============================================================"
echo "  Done. Results under $RESULTS_DIR"
if [[ -z "$APP_URL" ]]; then
  echo
  echo "  Phase 3 was skipped (no --app-url). To include live validation:"
  echo "    $0 --app-url https://<distribution>.cloudfront.net"
fi
echo "============================================================"
