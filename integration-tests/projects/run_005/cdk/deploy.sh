#!/usr/bin/env bash
set -euo pipefail

STACK_NAME="SecureRepoMonitoringStack-run003-3cbeaff7"
CONFIG_FILE="$(dirname "$0")/../config.json"

echo "==> Deploying CDK stack: $STACK_NAME"
npx cdk deploy "$STACK_NAME" --require-approval never --outputs-file /tmp/cdk-outputs.json

echo "==> Extracting stack outputs..."
TOPIC_ARN=$(jq -r ".\"$STACK_NAME\".TopicArn" /tmp/cdk-outputs.json)
SECRET_NAME=$(jq -r ".\"$STACK_NAME\".SecretName" /tmp/cdk-outputs.json)
SECRET_ARN=$(jq -r ".\"$STACK_NAME\".SecretArn" /tmp/cdk-outputs.json)
KMS_KEY_ID=$(jq -r ".\"$STACK_NAME\".KmsKeyId" /tmp/cdk-outputs.json)
KMS_KEY_ARN=$(jq -r ".\"$STACK_NAME\".KmsKeyArn" /tmp/cdk-outputs.json)
REPO_NAME=$(jq -r ".\"$STACK_NAME\".RepoName" /tmp/cdk-outputs.json)
CLONE_URL=$(jq -r ".\"$STACK_NAME\".CloneUrl" /tmp/cdk-outputs.json)
REGION=$(aws configure get region 2>/dev/null || echo "${AWS_DEFAULT_REGION:-us-east-1}")

echo "==> Writing $CONFIG_FILE"
cat > "$CONFIG_FILE" <<EOF
{
  "topicArn":   "$TOPIC_ARN",
  "secretName": "$SECRET_NAME",
  "secretArn":  "$SECRET_ARN",
  "kmsKeyId":   "$KMS_KEY_ID",
  "kmsKeyArn":  "$KMS_KEY_ARN",
  "repoName":   "$REPO_NAME",
  "cloneUrl":   "$CLONE_URL",
  "region":     "$REGION"
}
EOF

echo ""
echo "Stack deployed successfully."
echo "  SNS Topic   : $TOPIC_ARN"
echo "  Secret      : $SECRET_NAME"
echo "  KMS Key ID  : $KMS_KEY_ID"
echo "  Repo Name   : $REPO_NAME"
echo "  Clone URL   : $CLONE_URL"
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
