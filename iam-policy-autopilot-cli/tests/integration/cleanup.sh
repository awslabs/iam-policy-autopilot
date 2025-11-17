#!/bin/bash
set -e

# IAM Policy Autopilot - Standalone Cleanup Script
# Cleans up all resources created by the integration test

STACK_NAME="iam-autopilot-integration-test"
AWS_REGION="${AWS_REGION:-us-west-2}"
ROLE_NAME="IamAutopilotTestLambdaRole"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --region)
            AWS_REGION="$2"
            shift 2
            ;;
        --stack-name)
            STACK_NAME="$2"
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [--region REGION] [--stack-name NAME]"
            exit 0
            ;;
        *)
            log_error "Unknown option: $1"
            exit 1
            ;;
    esac
done

log_info "IAM Policy Autopilot - Cleanup Script"
log_info "Region: $AWS_REGION"
log_info "Stack: $STACK_NAME"

# Get account ID
ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
BUCKET_NAME="iam-autopilot-test-$ACCOUNT_ID"

# Delete inline policy
log_info "Deleting inline policy from role: $ROLE_NAME"
POLICY_NAME="IamPolicyAutopilot-$ROLE_NAME"

if aws iam get-role-policy \
    --role-name "$ROLE_NAME" \
    --policy-name "$POLICY_NAME" \
    --region "$AWS_REGION" &> /dev/null; then
    aws iam delete-role-policy \
        --role-name "$ROLE_NAME" \
        --policy-name "$POLICY_NAME" \
        --region "$AWS_REGION"
    log_success "Deleted inline policy: $POLICY_NAME"
else
    log_info "Inline policy not found (may already be deleted)"
fi

# Empty S3 bucket
log_info "Emptying S3 bucket: $BUCKET_NAME"
if aws s3 ls "s3://$BUCKET_NAME" --region "$AWS_REGION" &> /dev/null; then
    aws s3 rm "s3://$BUCKET_NAME" --recursive --region "$AWS_REGION"
    log_success "S3 bucket emptied"
else
    log_info "S3 bucket not found (may already be deleted)"
fi

# Note: DynamoDB table, KMS key, and Secrets Manager secret cleanup is handled by CloudFormation stack deletion
log_info "DynamoDB table cleanup will be handled by CloudFormation stack deletion"
log_info "KMS key and Secrets Manager secret cleanup will be handled by CloudFormation stack deletion"

# Delete CloudFormation stack
log_info "Deleting CloudFormation stack: $STACK_NAME"
if aws cloudformation describe-stacks \
    --stack-name "$STACK_NAME" \
    --region "$AWS_REGION" &> /dev/null; then
    aws cloudformation delete-stack \
        --stack-name "$STACK_NAME" \
        --region "$AWS_REGION"
    
    log_info "Waiting for stack deletion..."
    aws cloudformation wait stack-delete-complete \
        --stack-name "$STACK_NAME" \
        --region "$AWS_REGION" 2>&1 || {
        log_warn "Stack deletion wait timed out or failed"
        log_info "Check status: aws cloudformation describe-stacks --stack-name $STACK_NAME --region $AWS_REGION"
    }
    log_success "Stack deleted"
else
    log_info "Stack not found (may already be deleted)"
fi

log_success "Cleanup complete!"
