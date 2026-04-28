#!/usr/bin/env bash
# deploy.sh — Deploy the CDK stack and write config.json for all language scripts.
#
# Usage:
#   cd run_002/cdk
#   npm install          # first time only
#   npx cdk bootstrap    # first time per account/region
#   bash deploy.sh
#
# After deploy, run any language script — no arguments needed:
#   python  ../python/script.py
#   go run  ../go/script.go
#   (Java and TypeScript equivalents)
#
# To tear down:
#   npx cdk destroy

set -euo pipefail

STACK_NAME="FileMonitoringStack-run001"
CONFIG_FILE="$(dirname "$0")/../config.json"

echo "==> Deploying CDK stack: $STACK_NAME"
npx cdk deploy "$STACK_NAME" --require-approval never --outputs-file /tmp/cdk-outputs.json

echo "==> Extracting stack outputs..."
BUCKET_NAME=$(jq -r ".\"$STACK_NAME\".BucketName" /tmp/cdk-outputs.json)
QUEUE_URL=$(jq -r ".\"$STACK_NAME\".QueueUrl"   /tmp/cdk-outputs.json)
REGION=$(aws configure get region 2>/dev/null || echo "${AWS_DEFAULT_REGION:-us-east-1}")

echo "==> Writing $CONFIG_FILE"
cat > "$CONFIG_FILE" <<EOF
{
  "bucketName": "$BUCKET_NAME",
  "queueUrl":   "$QUEUE_URL",
  "region":     "$REGION"
}
EOF

echo ""
echo "Stack deployed successfully."
echo "  Bucket : $BUCKET_NAME"
echo "  Queue  : $QUEUE_URL"
echo "  Region : $REGION"
echo ""
echo "Run scripts:"
echo "  python  ../python/script.py"
echo "  go run  ../go/script.go"
echo ""
echo "Destroy when done:"
echo "  npx cdk destroy $STACK_NAME"
