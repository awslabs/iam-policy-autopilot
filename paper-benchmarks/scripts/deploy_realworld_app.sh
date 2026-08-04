#!/usr/bin/env bash
#
# Deploy the real-world benchmark application (aws-genai-llm-chatbot) from the
# Docker image to AWS.
#
# This script runs ON THE HOST (not inside the container) because CDK's Docker-
# based asset bundling requires the Docker daemon to access source paths on the
# filesystem — which doesn't work when CDK runs inside a container that shares
# the host's Docker socket.
#
# What it does:
#   1. Extracts the prepared genai-chatbot-cdk app from the Docker image
#   2. Runs npm install && npm run build
#   3. Bootstraps CDK (if needed) and deploys the stack
#
# Prerequisites (on the host):
#   - Docker (with ipa-paper-benchmarks image loaded)
#   - Node.js 18-20 and npm
#   - AWS CDK CLI: npm install -g aws-cdk
#   - AWS credentials configured (~/.aws/) with permissions in us-east-1
#
# Usage:
#   ./scripts/deploy_realworld_app.sh [work-dir]
#
# After deploy, run the evaluation inside the container:
#   docker run --rm \
#     -v ~/.aws:/root/.aws:ro \
#     -e AWS_DEFAULT_REGION=us-east-1 \
#     ipa-paper-benchmarks \
#     -c "./scripts/run_realworld_eval.sh --app-url https://<cloudfront-url>"
#
# To tear down:
#   cd <work-dir>/genai-chatbot-cdk && npx cdk destroy
#
set -euo pipefail

IMAGE_NAME="ipa-paper-benchmarks"
WORK_DIR="${1:-$(pwd)/realworld-deploy}"

# Check prerequisites
for cmd in docker node npm npx; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "error: '$cmd' not found on host. See REQUIREMENTS.md." >&2
        exit 1
    fi
done

if ! docker image inspect "$IMAGE_NAME" >/dev/null 2>&1; then
    echo "error: Docker image '$IMAGE_NAME' not found. Run: docker load < ipa-paper-benchmarks-image.tar.gz" >&2
    exit 1
fi

echo "==> Extracting genai-chatbot-cdk from Docker image to $WORK_DIR ..."
mkdir -p "$WORK_DIR"

docker run --rm \
    -v "$WORK_DIR:/out" \
    "$IMAGE_NAME" \
    -c "cp -a /opt/paper-benchmarks/real-world-apps/genai-chatbot-cdk /out/ && chown -R $(id -u):$(id -g) /out/genai-chatbot-cdk"

APP_DIR="$WORK_DIR/genai-chatbot-cdk"

if [[ ! -f "$APP_DIR/package.json" ]]; then
    echo "error: extraction failed — $APP_DIR/package.json not found" >&2
    exit 1
fi

echo "==> Installing dependencies ..."
cd "$APP_DIR"
npm install

echo "==> Building ..."
npm run build

echo "==> Bootstrapping CDK (if needed) ..."
export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-us-east-1}"
npx cdk bootstrap

echo "==> Deploying CDK stack (this may take 15-30 minutes) ..."
npx cdk deploy

CLOUDFRONT_URL=$(aws cloudformation describe-stacks \
    --stack-name GenAIChatBotStack --region us-east-1 \
    --query "Stacks[0].Outputs[?OutputKey=='metadata'].OutputValue" \
    --output text 2>/dev/null | jq -r '.ChatbotUserInterfaceDomainName' 2>/dev/null || echo "https://<cloudfront-url>")

echo
echo "============================================================"
echo "  Deployment complete!"
echo "  App URL: $CLOUDFRONT_URL"
echo "============================================================"
echo
echo "  Smoke test (single handler, IPA only):"
echo
echo "    docker run --rm \\"
echo "      -v ~/.aws:/root/.aws:ro \\"
echo "      -e AWS_DEFAULT_REGION=us-east-1 \\"
echo "      ipa-paper-benchmarks \\"
echo "      -c \"MODE=ipa ./scripts/run_realworld_eval.sh --app-url $CLOUDFRONT_URL --only api-handler\""
echo
echo "  Full run (all handlers, all modes — ~10 hours):"
echo
echo "    docker run --rm \\"
echo "      -v ~/.aws:/root/.aws:ro \\"
echo "      -e AWS_DEFAULT_REGION=us-east-1 \\"
echo "      -v \$(pwd)/results:/opt/paper-benchmarks/real-world-apps/realworld_results \\"
echo "      ipa-paper-benchmarks \\"
echo "      -c \"./scripts/run_realworld_eval.sh --app-url $CLOUDFRONT_URL\""
echo
echo "  To tear down when finished:"
echo "    cd $APP_DIR && npx cdk destroy"
echo
