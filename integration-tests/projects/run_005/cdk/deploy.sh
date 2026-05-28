#!/usr/bin/env bash
set -euo pipefail

STACK_NAME="SecureRepoMonitoringStack-run003-3cbeaff7"
CONFIG_FILE="$(dirname "$0")/../config.json"
CDK_OUTPUTS=$(mktemp "${TMPDIR:-/tmp}/cdk-outputs.XXXXXXXXXX.json")
trap 'rm -f "$CDK_OUTPUTS"' EXIT

echo "==> Deploying CDK stack: $STACK_NAME"
npx cdk deploy "$STACK_NAME" --require-approval never --outputs-file "$CDK_OUTPUTS"

echo "==> Extracting stack outputs..."
TOPIC_ARN=$(jq -r ".\"$STACK_NAME\".TopicArn" "$CDK_OUTPUTS")
SECRET_NAME=$(jq -r ".\"$STACK_NAME\".SecretName" "$CDK_OUTPUTS")
SECRET_ARN=$(jq -r ".\"$STACK_NAME\".SecretArn" "$CDK_OUTPUTS")
KMS_KEY_ID=$(jq -r ".\"$STACK_NAME\".KmsKeyId" "$CDK_OUTPUTS")
KMS_KEY_ARN=$(jq -r ".\"$STACK_NAME\".KmsKeyArn" "$CDK_OUTPUTS")
REPO_NAME=$(jq -r ".\"$STACK_NAME\".RepoName" "$CDK_OUTPUTS")
CLONE_URL=$(jq -r ".\"$STACK_NAME\".CloneUrl" "$CDK_OUTPUTS")
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
