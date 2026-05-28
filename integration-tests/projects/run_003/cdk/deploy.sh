#!/usr/bin/env bash
# deploy.sh — Deploy the CDK stack and write config.json for all language scripts.
#
# Usage:
#   cd run_003/cdk
#   npm install          # first time only
#   npx cdk bootstrap    # first time per account/region
#   bash deploy.sh
#
# After deploy, run any language script — no arguments needed:
#   python  ../python/script.py
#   go run  ../go/script.go
#   cd ../java && mvn exec:java -Dexec.mainClass=Script
#   cd ../typescript && npm install && npx ts-node script.ts
#
# To tear down:
#   npx cdk destroy

set -euo pipefail

STACK_NAME="DataPipelineStack-run002-7beb16a2"
CONFIG_FILE="$(dirname "$0")/../config.json"
CDK_OUTPUTS=$(mktemp "${TMPDIR:-/tmp}/cdk-outputs.XXXXXXXXXX.json")
trap 'rm -f "$CDK_OUTPUTS"' EXIT

echo "==> Deploying CDK stack: $STACK_NAME"
npx cdk deploy "$STACK_NAME" --require-approval never --outputs-file "$CDK_OUTPUTS"

echo "==> Extracting stack outputs..."
BUCKET_NAME=$(jq -r ".\"$STACK_NAME\".BucketName"      "$CDK_OUTPUTS")
KMS_KEY_ID=$(jq -r ".\"$STACK_NAME\".KmsKeyId"         "$CDK_OUTPUTS")
KMS_KEY_ARN=$(jq -r ".\"$STACK_NAME\".KmsKeyArn"       "$CDK_OUTPUTS")
STATE_MACHINE_ARN=$(jq -r ".\"$STACK_NAME\".StateMachineArn" "$CDK_OUTPUTS")
LOG_GROUP_NAME=$(jq -r ".\"$STACK_NAME\".LogGroupName" "$CDK_OUTPUTS")
REGION=$(aws configure get region 2>/dev/null || echo "${AWS_DEFAULT_REGION:-us-east-1}")

echo "==> Writing $CONFIG_FILE"
cat > "$CONFIG_FILE" <<EOF
{
  "bucketName":       "$BUCKET_NAME",
  "kmsKeyId":         "$KMS_KEY_ID",
  "kmsKeyArn":        "$KMS_KEY_ARN",
  "stateMachineArn":  "$STATE_MACHINE_ARN",
  "logGroupName":     "$LOG_GROUP_NAME",
  "region":           "$REGION"
}
EOF

echo ""
echo "Stack deployed successfully."
echo "  Bucket          : $BUCKET_NAME"
echo "  KMS Key ID      : $KMS_KEY_ID"
echo "  State Machine   : $STATE_MACHINE_ARN"
echo "  Log Group       : $LOG_GROUP_NAME"
echo "  Region          : $REGION"
echo ""
echo "Run scripts:"
echo "  python  ../python/script.py"
echo "  go run  ../go/script.go"
echo "  cd ../java && mvn exec:java -Dexec.mainClass=Script"
echo "  cd ../typescript && npm install && npx ts-node script.ts"
echo ""
echo "Destroy when done:"
echo "  npx cdk destroy $STACK_NAME"
