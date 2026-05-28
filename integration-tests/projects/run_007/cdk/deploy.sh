#!/usr/bin/env bash
set -euo pipefail

STACK_NAME="ComprehensiveMonitoringStack-run004-897f3738"
CONFIG_FILE="$(dirname "$0")/../config.json"
CDK_OUTPUTS=$(mktemp "${TMPDIR:-/tmp}/cdk-outputs.XXXXXXXXXX.json")
trap 'rm -f "$CDK_OUTPUTS"' EXIT

echo "==> Deploying CDK stack: $STACK_NAME"
npx cdk deploy "$STACK_NAME" --require-approval never --outputs-file "$CDK_OUTPUTS"

echo "==> Extracting stack outputs..."
BUCKET_NAME=$(jq -r ".\"$STACK_NAME\".BucketName" "$CDK_OUTPUTS")
REGION=$(aws configure get region 2>/dev/null || echo "${AWS_DEFAULT_REGION:-us-east-1}")

echo "==> Writing $CONFIG_FILE"
cat > "$CONFIG_FILE" <<EOF
{
  "bucketName": "$BUCKET_NAME",
  "region":     "$REGION"
}
EOF

echo "Stack deployed. Bucket: $BUCKET_NAME"
