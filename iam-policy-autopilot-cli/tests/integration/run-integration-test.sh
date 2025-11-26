#!/bin/bash
set -e

# IAM Policy Autopilot - Integration Test Runner
# This script deploys a Lambda function, triggers AccessDenied errors,
# uses the autopilot tool to fix permissions, and verifies the fix works.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
AUTOPILOT_BIN="$PROJECT_ROOT/target/release/iam-policy-autopilot"

# Test configuration
STACK_NAME="iam-autopilot-integration-test"
export AWS_REGION="${AWS_REGION:-us-west-2}"
CLEANUP_MODE="auto"  # auto, manual, or keep

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Logging functions
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

# Usage information
usage() {
    cat << EOF
Usage: $0 [OPTIONS]

Integration test for IAM Policy Autopilot

OPTIONS:
    --cleanup       Run cleanup and exit (default behavior after test)
    --no-cleanup    Skip cleanup after test (for debugging)
    --keep          Keep all resources after test (no cleanup)
    --region REGION AWS region (default: us-west-2)
    --help          Show this help message

EXAMPLES:
    $0                      # Run full test with automatic cleanup
    $0 --no-cleanup         # Run test, keep resources for inspection
    $0 --cleanup            # Just run cleanup
    $0 --region us-east-1   # Run test in different region

EOF
    exit 1
}

# Parse command line arguments
parse_args() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            --cleanup)
                CLEANUP_MODE="cleanup-only"
                shift
                ;;
            --no-cleanup)
                CLEANUP_MODE="manual"
                shift
                ;;
            --keep)
                CLEANUP_MODE="keep"
                shift
                ;;
            --region)
                AWS_REGION="$2"
                shift 2
                ;;
            --help|-h)
                usage
                ;;
            *)
                log_error "Unknown option: $1"
                usage
                ;;
        esac
    done
}

# Pre-flight checks
preflight_checks() {
    log_info "Running pre-flight checks..."
    
    # Check if autopilot binary exists
    if [ ! -f "$AUTOPILOT_BIN" ]; then
        log_error "Autopilot binary not found at: $AUTOPILOT_BIN"
        log_info "Building autopilot..."
        cd "$PROJECT_ROOT"
        cargo build -p iam-policy-autopilot --release
        if [ ! -f "$AUTOPILOT_BIN" ]; then
            log_error "Failed to build autopilot binary"
            exit 1
        fi
    fi
    
    # Check AWS CLI
    if ! command -v aws &> /dev/null; then
        log_error "AWS CLI not found. Please install it."
        exit 1
    fi
    
    # Check jq
    if ! command -v jq &> /dev/null; then
        log_error "jq not found. Please install it (brew install jq or apt-get install jq)."
        exit 1
    fi
    
    # Check AWS credentials
    if ! aws sts get-caller-identity --region "$AWS_REGION" &> /dev/null; then
        log_error "AWS credentials not configured or invalid"
        exit 1
    fi
    
    # Check for existing stack (warn if exists)
    if aws cloudformation describe-stacks --stack-name "$STACK_NAME" --region "$AWS_REGION" &> /dev/null; then
        log_warn "Stack $STACK_NAME already exists. Running pre-cleanup..."
        run_cleanup
    fi
    
    log_success "Pre-flight checks passed"
}

# Deploy CloudFormation stack
deploy_stack() {
    log_info "Deploying CloudFormation stack: $STACK_NAME"
    
    aws cloudformation create-stack \
        --stack-name "$STACK_NAME" \
        --template-body "file://$SCRIPT_DIR/cfn-lambda-test.yaml" \
        --capabilities CAPABILITY_NAMED_IAM \
        --region "$AWS_REGION" \
        --output text
    
    log_info "Waiting for stack creation to complete..."
    aws cloudformation wait stack-create-complete \
        --stack-name "$STACK_NAME" \
        --region "$AWS_REGION"
    
    log_success "Stack deployed successfully"
}

# Get stack outputs
get_stack_outputs() {
    log_info "Retrieving stack outputs..."
    
    ROLE_ARN=$(aws cloudformation describe-stacks \
        --stack-name "$STACK_NAME" \
        --region "$AWS_REGION" \
        --query 'Stacks[0].Outputs[?OutputKey==`LambdaRoleArn`].OutputValue' \
        --output text)
    
    FUNCTION_NAME=$(aws cloudformation describe-stacks \
        --stack-name "$STACK_NAME" \
        --region "$AWS_REGION" \
        --query 'Stacks[0].Outputs[?OutputKey==`LambdaFunctionName`].OutputValue' \
        --output text)
    
    BUCKET_NAME=$(aws cloudformation describe-stacks \
        --stack-name "$STACK_NAME" \
        --region "$AWS_REGION" \
        --query 'Stacks[0].Outputs[?OutputKey==`TestBucketName`].OutputValue' \
        --output text)
    
    ROLE_NAME=$(aws cloudformation describe-stacks \
        --stack-name "$STACK_NAME" \
        --region "$AWS_REGION" \
        --query 'Stacks[0].Outputs[?OutputKey==`LambdaRoleName`].OutputValue' \
        --output text)
    
    log_info "Role ARN: $ROLE_ARN"
    log_info "Function Name: $FUNCTION_NAME"
    log_info "Bucket Name: $BUCKET_NAME"
    log_info "Role Name: $ROLE_NAME"
}

# Invoke Lambda and get logs
invoke_lambda_and_get_logs() {
    log_info "Invoking Lambda function..."
    
    # Invoke Lambda with tail logs
    aws lambda invoke \
        --function-name "$FUNCTION_NAME" \
        --region "$AWS_REGION" \
        --log-type Tail \
        /tmp/lambda-response.json > /tmp/lambda-invoke.json
    
    # Extract and decode tail logs from invoke response
    LOG_RESULT=$(jq -r '.LogResult' /tmp/lambda-invoke.json)
    
    if [ -z "$LOG_RESULT" ] || [ "$LOG_RESULT" = "null" ]; then
        log_error "No LogResult found in Lambda invoke response"
        log_info "Invoke response:"
        cat /tmp/lambda-invoke.json
        exit 1
    fi
    
    echo "$LOG_RESULT" | base64 --decode > /tmp/lambda-logs.txt
    
    log_info "Lambda logs preview (first 15 lines):"
    head -15 /tmp/lambda-logs.txt
}

# Run autopilot to fix permissions (pass complete unfiltered logs)
run_autopilot_fix() {
    log_info "Running IAM Policy Autopilot (passing complete Lambda logs)..."
    
    # Pass FULL unfiltered logs to autopilot via stdin
    if cat /tmp/lambda-logs.txt | "$AUTOPILOT_BIN" fix-access-denied --yes; then
        log_success "Autopilot successfully applied permission fix"
    else
        EXIT_CODE=$?
        if [ $EXIT_CODE -eq 1 ]; then
            log_warn "Duplicate statement detected (exit code 1)"
        else
            log_error "Autopilot failed with exit code $EXIT_CODE"
            exit 1
        fi
    fi
}

# Verify and iteratively fix all permissions
verify_and_fix_all() {
    log_info "Starting iterative permission fix process..."
    
    local MAX_ITERATIONS=15
    local iteration=1
    
    while [ $iteration -le $MAX_ITERATIONS ]; do
        log_info "=== Iteration $iteration: Testing permissions ==="
        
        # Wait for IAM propagation (except first iteration)
        if [ $iteration -gt 1 ]; then
            log_info "Waiting 10 seconds for IAM policy propagation..."
            sleep 10
        fi
        
        # Invoke Lambda
        aws lambda invoke \
            --function-name "$FUNCTION_NAME" \
            --region "$AWS_REGION" \
            --log-type Tail \
            "/tmp/lambda-response-$iteration.json" > "/tmp/lambda-invoke-$iteration.json"
        
        # Extract and decode logs
        LOG_RESULT=$(jq -r '.LogResult' "/tmp/lambda-invoke-$iteration.json")
        
        if [ -z "$LOG_RESULT" ] || [ "$LOG_RESULT" = "null" ]; then
            log_error "No LogResult found in Lambda invoke response"
            exit 1
        fi
        
        echo "$LOG_RESULT" | base64 --decode > "/tmp/lambda-logs-$iteration.txt"
        
        # Check Lambda response body for success (statusCode 200 in body)
        BODY=$(cat "/tmp/lambda-response-$iteration.json")
        if echo "$BODY" | jq -e '.statusCode == 200' > /dev/null 2>&1; then
            log_success "✅ Lambda completed successfully - all permissions granted!"
            log_info "Total iterations needed: $iteration"
            return 0
        fi
        
        # Lambda failed - check for AccessDenied in logs
        if ! grep -q "is not authorized to perform" "/tmp/lambda-logs-$iteration.txt"; then
            log_error "Lambda failed but no AccessDenied error found in logs"
            log_info "Lambda response:"
            cat "/tmp/lambda-response-$iteration.json"
            log_info "Lambda logs:"
            cat "/tmp/lambda-logs-$iteration.txt"
            exit 1
        fi
        
        log_info "Lambda failed with AccessDenied error"
        log_info "Log preview:"
        head -10 "/tmp/lambda-logs-$iteration.txt"
        
        # Pass COMPLETE unfiltered logs to autopilot
        log_info "Running autopilot with full Lambda logs..."
        
        if cat "/tmp/lambda-logs-$iteration.txt" | "$AUTOPILOT_BIN" fix-access-denied --yes; then
            log_success "Autopilot applied permission fix #$iteration"
        else
            EXIT_CODE=$?
            if [ $EXIT_CODE -eq 1 ]; then
                log_warn "Duplicate detected (may indicate issue)"
            else
                log_error "Autopilot failed with exit code $EXIT_CODE"
                exit 1
            fi
        fi
        
        iteration=$((iteration + 1))
    done
    
    log_error "Max iterations ($MAX_ITERATIONS) reached - Lambda still failing"
    exit 1
}

# Test idempotency (duplicate detection)
test_idempotency() {
    log_info "Testing idempotency (running autopilot again with first error logs)..."
    
    # Run autopilot again with original logs - should detect duplicate
    if cat /tmp/lambda-logs.txt | "$AUTOPILOT_BIN" fix-access-denied --yes; then
        log_error "Expected duplicate detection (exit code 1), got success (exit code 0)"
        exit 1
    else
        EXIT_CODE=$?
        if [ $EXIT_CODE -eq 1 ]; then
            log_success "Idempotency test passed - duplicate statement detected"
        else
            log_error "Unexpected exit code: $EXIT_CODE (expected 1 for duplicate)"
            exit 1
        fi
    fi
}

# Run cleanup
run_cleanup() {
    log_info "Starting cleanup..."
    
    # Set default role name if not already set
    if [ -z "$ROLE_NAME" ]; then
        ROLE_NAME="IamAutopilotTestLambdaRole"
    fi
    
    # Delete ALL inline policies from the role (autopilot-created policies)
    log_info "Cleaning up inline policies from role: $ROLE_NAME"
    POLICY_NAMES=$(aws iam list-role-policies \
        --role-name "$ROLE_NAME" \
        --region "$AWS_REGION" \
        --query 'PolicyNames[]' \
        --output text 2>/dev/null || echo "")
    
    if [ -n "$POLICY_NAMES" ]; then
        for POLICY_NAME in $POLICY_NAMES; do
            log_info "Deleting inline policy: $POLICY_NAME"
            aws iam delete-role-policy \
                --role-name "$ROLE_NAME" \
                --policy-name "$POLICY_NAME" \
                --region "$AWS_REGION"
            log_success "Deleted inline policy: $POLICY_NAME"
        done
    else
        log_info "No inline policies found on role"
    fi
    
    # Empty S3 bucket if it exists
    if [ -z "$BUCKET_NAME" ]; then
        BUCKET_NAME="iam-autopilot-test-$(aws sts get-caller-identity --query Account --output text)"
    fi
    
    if aws s3 ls "s3://$BUCKET_NAME" --region "$AWS_REGION" &> /dev/null; then
        log_info "Emptying S3 bucket: $BUCKET_NAME"
        aws s3 rm "s3://$BUCKET_NAME" --recursive --region "$AWS_REGION" || true
        log_success "S3 bucket emptied"
    else
        log_info "S3 bucket not found or already deleted"
    fi
    
    # Delete CloudFormation stack (role will be retained due to DeletionPolicy)
    log_info "Deleting CloudFormation stack: $STACK_NAME"
    log_info "Note: IAM role will be retained (DeletionPolicy: Retain)"
    aws cloudformation delete-stack \
        --stack-name "$STACK_NAME" \
        --region "$AWS_REGION"
    
    log_info "Waiting for stack deletion..."
    aws cloudformation wait stack-delete-complete \
        --stack-name "$STACK_NAME" \
        --region "$AWS_REGION" || {
        log_warn "Stack deletion wait failed or timed out"
        log_info "Check stack status with: aws cloudformation describe-stacks --stack-name $STACK_NAME --region $AWS_REGION"
    }
    
    log_success "Cleanup complete (IAM role retained for reuse)"
}

# Main test flow
main() {
    parse_args "$@"
    
    # Handle cleanup-only mode
    if [ "$CLEANUP_MODE" = "cleanup-only" ]; then
        log_info "Running cleanup only..."
        get_stack_outputs || {
            log_warn "Could not get stack outputs, attempting cleanup anyway"
            ROLE_NAME="IamAutopilotTestLambdaRole"
            BUCKET_NAME="iam-autopilot-test-$(aws sts get-caller-identity --query Account --output text)"
        }
        run_cleanup
        exit 0
    fi
    
    log_info "Starting IAM Policy Autopilot Integration Test"
    log_info "Region: $AWS_REGION"
    log_info "Cleanup mode: $CLEANUP_MODE"
    
    # Set up error handling
    trap 'log_error "Test failed. Run with --keep to preserve resources for debugging."; [ "$CLEANUP_MODE" = "auto" ] && run_cleanup' ERR
    
    # Run test steps
    preflight_checks
    deploy_stack
    get_stack_outputs
    invoke_lambda_and_get_logs
    run_autopilot_fix
    verify_and_fix_all
    test_idempotency
    
    log_success "All tests passed!"
    
    # Handle cleanup based on mode
    case "$CLEANUP_MODE" in
        auto)
            log_info "Running automatic cleanup..."
            run_cleanup
            ;;
        keep)
            log_info "Keeping all resources (--keep mode)"
            log_info "To clean up later, run: $0 --cleanup"
            ;;
        manual)
            log_info "Skipping cleanup (--no-cleanup mode)"
            log_info "To clean up, run: $0 --cleanup"
            ;;
    esac
    
    log_success "Integration test complete!"
}

# Run main function
main "$@"
