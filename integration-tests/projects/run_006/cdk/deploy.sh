#!/usr/bin/env bash
set -euo pipefail

STACK_NAME="SecureDocumentStack-run003-849526af"
CONFIG_FILE="$(dirname "$0")/../config.json"

echo "==> Deploying CDK stack: $STACK_NAME"
npx cdk deploy "$STACK_NAME" --require-approval never --outputs-file /tmp/cdk-outputs.json

echo "==> Extracting stack outputs..."
BUCKET_NAME=$(jq -r ".\"$STACK_NAME\".BucketName"    /tmp/cdk-outputs.json)
TABLE_NAME=$(jq -r ".\"$STACK_NAME\".TableName"      /tmp/cdk-outputs.json)
KMS_KEY_ID=$(jq -r ".\"$STACK_NAME\".KmsKeyId"       /tmp/cdk-outputs.json)
KMS_KEY_ARN=$(jq -r ".\"$STACK_NAME\".KmsKeyArn"     /tmp/cdk-outputs.json)
KMS_ALIAS=$(jq -r ".\"$STACK_NAME\".KmsAlias"        /tmp/cdk-outputs.json)
LOG_GROUP_NAME=$(jq -r ".\"$STACK_NAME\".LogGroupName" /tmp/cdk-outputs.json)
REGION=$(aws configure get region 2>/dev/null || echo "${AWS_DEFAULT_REGION:-us-east-1}")

echo "==> Writing $CONFIG_FILE"
cat > "$CONFIG_FILE" <<EOF
{
  "bucketName":   "$BUCKET_NAME",
  "tableName":    "$TABLE_NAME",
  "kmsKeyId":     "$KMS_KEY_ID",
  "kmsKeyArn":    "$KMS_KEY_ARN",
  "kmsAlias":     "$KMS_ALIAS",
  "logGroupName": "$LOG_GROUP_NAME",
  "region":       "$REGION"
}
EOF

echo ""
echo "Stack deployed successfully."
echo "  S3 Bucket   : $BUCKET_NAME"
echo "  DynamoDB    : $TABLE_NAME"
echo "  KMS Key ID  : $KMS_KEY_ID"
echo "  Log Group   : $LOG_GROUP_NAME"
echo "  Region      : $REGION"
echo ""
echo "Run scripts:"
echo "  python  ../python/script.py"
echo "  go run  ../go/script.go"
echo "  cd ../java && mvn exec:java -Dexec.mainClass=Script"
echo "  cd ../typescript && npm install && npx ts-node script.ts"
echo ""
echo "Destroy when done:"
echo "  npx cdk destroy $STACK_NAME"
