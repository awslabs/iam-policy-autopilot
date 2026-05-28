#!/usr/bin/env bash
set -euo pipefail

STACK_NAME="ServiceCatalogManagerStack-run004-a6a046d0"
CONFIG_FILE="$(dirname "$0")/../config.json"
CDK_OUTPUTS=$(mktemp "${TMPDIR:-/tmp}/cdk-outputs.XXXXXXXXXX.json")
trap 'rm -f "$CDK_OUTPUTS"' EXIT

echo "==> Deploying CDK stack: $STACK_NAME"
npx cdk deploy "$STACK_NAME" --require-approval never --outputs-file "$CDK_OUTPUTS"

echo "==> Extracting stack outputs..."
LOG_GROUP_NAME=$(jq -r ".\"$STACK_NAME\".LogGroupName" "$CDK_OUTPUTS")
REGION=$(aws configure get region 2>/dev/null || echo "${AWS_DEFAULT_REGION:-us-east-1}")

echo "==> Writing $CONFIG_FILE"
cat > "$CONFIG_FILE" <<EOF
{
  "logGroupName": "$LOG_GROUP_NAME",
  "region":       "$REGION"
}
EOF

echo "Stack deployed. Log Group: $LOG_GROUP_NAME"
