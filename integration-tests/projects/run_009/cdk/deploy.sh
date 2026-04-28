#!/usr/bin/env bash
set -euo pipefail

STACK_NAME="ComplianceMonitoringStack-run005-98c7d54c"
CONFIG_FILE="$(dirname "$0")/../config.json"

echo "==> Deploying CDK stack: $STACK_NAME"
npx cdk deploy "$STACK_NAME" --require-approval never --outputs-file /tmp/cdk-outputs.json

echo "==> Extracting stack outputs..."
BUCKET_NAME=$(jq -r ".\"$STACK_NAME\".BucketName" /tmp/cdk-outputs.json)
KMS_KEY_ID=$(jq -r ".\"$STACK_NAME\".KmsKeyId" /tmp/cdk-outputs.json)
KMS_KEY_ARN=$(jq -r ".\"$STACK_NAME\".KmsKeyArn" /tmp/cdk-outputs.json)
REGION=$(aws configure get region 2>/dev/null || echo "${AWS_DEFAULT_REGION:-us-east-1}")

echo "==> Writing $CONFIG_FILE"
cat > "$CONFIG_FILE" <<EOF
{
  "bucketName": "$BUCKET_NAME",
  "kmsKeyId":   "$KMS_KEY_ID",
  "kmsKeyArn":  "$KMS_KEY_ARN",
  "region":     "$REGION"
}
EOF

echo "Stack deployed. Bucket: $BUCKET_NAME, KMS Key: $KMS_KEY_ID"
