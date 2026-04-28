#!/usr/bin/env bash
set -euo pipefail

STACK_NAME="ComprehensiveMonitoringStack-run004-897f3738"
CONFIG_FILE="$(dirname "$0")/../config.json"

echo "==> Deploying CDK stack: $STACK_NAME"
npx cdk deploy "$STACK_NAME" --require-approval never --outputs-file /tmp/cdk-outputs.json

echo "==> Extracting stack outputs..."
BUCKET_NAME=$(jq -r ".\"$STACK_NAME\".BucketName" /tmp/cdk-outputs.json)
REGION=$(aws configure get region 2>/dev/null || echo "${AWS_DEFAULT_REGION:-us-east-1}")

echo "==> Writing $CONFIG_FILE"
cat > "$CONFIG_FILE" <<EOF
{
  "bucketName": "$BUCKET_NAME",
  "region":     "$REGION"
}
EOF

echo "Stack deployed. Bucket: $BUCKET_NAME"
