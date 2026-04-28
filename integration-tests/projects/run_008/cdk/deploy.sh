#!/usr/bin/env bash
set -euo pipefail

STACK_NAME="ServiceCatalogManagerStack-run004-a6a046d0"
CONFIG_FILE="$(dirname "$0")/../config.json"

echo "==> Deploying CDK stack: $STACK_NAME"
npx cdk deploy "$STACK_NAME" --require-approval never --outputs-file /tmp/cdk-outputs.json

echo "==> Extracting stack outputs..."
LOG_GROUP_NAME=$(jq -r ".\"$STACK_NAME\".LogGroupName" /tmp/cdk-outputs.json)
REGION=$(aws configure get region 2>/dev/null || echo "${AWS_DEFAULT_REGION:-us-east-1}")

echo "==> Writing $CONFIG_FILE"
cat > "$CONFIG_FILE" <<EOF
{
  "logGroupName": "$LOG_GROUP_NAME",
  "region":       "$REGION"
}
EOF

echo "Stack deployed. Log Group: $LOG_GROUP_NAME"
