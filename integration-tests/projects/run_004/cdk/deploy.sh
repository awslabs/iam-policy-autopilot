#!/usr/bin/env bash
set -euo pipefail

STACK_NAME="MLMonitoringStack-run002-b82fea53"
CONFIG_FILE="$(dirname "$0")/../config.json"
CDK_OUTPUTS=$(mktemp "${TMPDIR:-/tmp}/cdk-outputs.XXXXXXXXXX.json")
trap 'rm -f "$CDK_OUTPUTS"' EXIT

echo "==> Deploying CDK stack: $STACK_NAME"
npx cdk deploy "$STACK_NAME" --require-approval never --outputs-file "$CDK_OUTPUTS"

echo "==> Extracting stack outputs..."
CLUSTER_NAME=$(jq -r ".\"$STACK_NAME\".ClusterName" "$CDK_OUTPUTS")
CLUSTER_ARN=$(jq -r ".\"$STACK_NAME\".ClusterArn" "$CDK_OUTPUTS")
LOG_GROUP_NAME=$(jq -r ".\"$STACK_NAME\".LogGroupName" "$CDK_OUTPUTS")
KMS_KEY_ID=$(jq -r ".\"$STACK_NAME\".KmsKeyId" "$CDK_OUTPUTS")
KMS_KEY_ARN=$(jq -r ".\"$STACK_NAME\".KmsKeyArn" "$CDK_OUTPUTS")
RESOURCE_GROUP_NAME=$(jq -r ".\"$STACK_NAME\".ResourceGroupName" "$CDK_OUTPUTS")
REGION=$(aws configure get region 2>/dev/null || echo "${AWS_DEFAULT_REGION:-us-east-1}")

echo "==> Writing $CONFIG_FILE"
cat > "$CONFIG_FILE" <<EOF
{
  "clusterName":       "$CLUSTER_NAME",
  "clusterArn":        "$CLUSTER_ARN",
  "logGroupName":      "$LOG_GROUP_NAME",
  "kmsKeyId":          "$KMS_KEY_ID",
  "kmsKeyArn":         "$KMS_KEY_ARN",
  "resourceGroupName": "$RESOURCE_GROUP_NAME",
  "region":            "$REGION"
}
EOF

echo ""
echo "Stack deployed successfully."
echo "  ECS Cluster     : $CLUSTER_NAME"
echo "  Log Group       : $LOG_GROUP_NAME"
echo "  KMS Key ID      : $KMS_KEY_ID"
echo "  Resource Group  : $RESOURCE_GROUP_NAME"
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
