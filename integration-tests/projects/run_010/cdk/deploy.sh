#!/usr/bin/env bash
set -euo pipefail

STACK_NAME="DeploymentMonitoringStack-run005-9a05981d"
CONFIG_FILE="$(dirname "$0")/../config.json"

echo "==> Deploying CDK stack: $STACK_NAME"
npx cdk deploy "$STACK_NAME" --require-approval never --outputs-file /tmp/cdk-outputs.json

echo "==> Extracting stack outputs..."
FUNCTION_NAME=$(jq -r ".\"$STACK_NAME\".FunctionName" /tmp/cdk-outputs.json)
LOG_GROUP_NAME=$(jq -r ".\"$STACK_NAME\".LogGroupName" /tmp/cdk-outputs.json)
REGION=$(aws configure get region 2>/dev/null || echo "${AWS_DEFAULT_REGION:-us-east-1}")

echo "==> Writing $CONFIG_FILE"
cat > "$CONFIG_FILE" <<EOF
{
  "functionName": "$FUNCTION_NAME",
  "logGroupName": "$LOG_GROUP_NAME",
  "region":       "$REGION"
}
EOF

echo "Stack deployed. Function: $FUNCTION_NAME, Log Group: $LOG_GROUP_NAME"
