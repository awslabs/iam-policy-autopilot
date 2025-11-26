# IAM Policy Autopilot - Integration Test Guide

Complete guide for running integration tests that verify IAM Policy Autopilot in real AWS environments.

## Overview

The integration test:
1. Deploys a Lambda function with insufficient S3 permissions
2. Invokes the Lambda (triggering AccessDenied errors)
3. Extracts errors from CloudWatch Logs
4. Uses IAM Policy Autopilot to automatically fix permissions
5. Verifies the fix works by re-invoking Lambda
6. Tests idempotency (duplicate detection)
7. Cleans up all resources

## Prerequisites

### Required Tools
- AWS CLI (configured with credentials)
- Rust toolchain (for building autopilot)
- Bash shell (Linux, macOS, or WSL on Windows)

### AWS Permissions
Your AWS credentials need the following permissions:
```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": [
        "cloudformation:*",
        "lambda:*",
        "s3:*",
        "iam:*",
        "logs:*",
        "sts:GetCallerIdentity"
      ],
      "Resource": "*"
    }
  ]
}
```

### Build Autopilot
```bash
cd iam-policy-autopilot
cargo build -p iam-policy-autopilot --release
```

## Quick Start

### Run Complete Test (with automatic cleanup)
```bash
cd iam-policy-autopilot/tests/integration
./run-integration-test.sh
```

### Run Test (keep resources for inspection)
```bash
./run-integration-test.sh --no-cleanup
```

### Run Test (keep resources, no cleanup at all)
```bash
./run-integration-test.sh --keep
```

### Run Only Cleanup
```bash
./run-integration-test.sh --cleanup
# OR
./cleanup.sh
```

## Test Workflow

### Step 1: Pre-flight Checks
- Verifies autopilot binary exists (builds if needed)
- Checks AWS CLI and credentials
- Warns if stack already exists (runs pre-cleanup)

**Expected output:**
```
[INFO] Running pre-flight checks...
[SUCCESS] Pre-flight checks passed
```

### Step 2: Deploy CloudFormation Stack
Creates:
- Lambda function (Python 3.12) that attempts S3 operations
- IAM execution role with ONLY CloudWatch Logs permissions
- S3 bucket for testing

**Expected output:**
```
[INFO] Deploying CloudFormation stack: iam-autopilot-integration-test
[INFO] Waiting for stack creation to complete...
[SUCCESS] Stack deployed successfully
```

### Step 3: Invoke Lambda and Extract Errors
- Invokes Lambda function
- Waits for CloudWatch Logs
- Extracts AccessDenied error message

**Expected output:**
```
[INFO] Invoking Lambda function (expecting AccessDenied errors)...
[SUCCESS] Found AccessDenied error:
User: arn:aws:sts::123456789012:assumed-role/IamAutopilotTestLambdaRole/IamAutopilotTestFunction is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::iam-autopilot-test-123456789012/test-object.txt
```

### Step 4: Run Autopilot Fix
- Runs `iam-policy-autopilot fix-access-denied` with the error
- Creates inline IAM policy on the Lambda role
- Adds necessary S3 permissions

**Expected output:**
```
[INFO] Running IAM Policy Autopilot to fix permissions...
[SUCCESS] Autopilot successfully applied fix
```

### Step 5: Verify Fix
- Waits 10 seconds for IAM policy propagation
- Re-invokes Lambda function
- Checks CloudWatch Logs for errors

**Expected output:**
```
[INFO] Verifying the fix by re-invoking Lambda...
[INFO] Waiting 10 seconds for IAM policy propagation...
[SUCCESS] Verification passed - no more AccessDenied errors
```

### Step 6: Test Idempotency
- Runs autopilot again with the same error
- Verifies duplicate detection (exit code 1)

**Expected output:**
```
[INFO] Testing idempotency (running autopilot again with same error)...
[SUCCESS] Idempotency test passed - duplicate statement detected
```

### Step 7: Cleanup
- Deletes inline IAM policy
- Empties S3 bucket
- Deletes CloudFormation stack

**Expected output:**
```
[INFO] Running automatic cleanup...
[INFO] Starting cleanup...
[SUCCESS] Deleted inline policy
[SUCCESS] S3 bucket emptied
[SUCCESS] Cleanup complete
```

## Command Line Options

### `run-integration-test.sh`

| Option | Description |
|--------|-------------|
| `--cleanup` | Run cleanup only and exit |
| `--no-cleanup` | Skip cleanup after test (for debugging) |
| `--keep` | Keep all resources after test (no cleanup) |
| `--region REGION` | AWS region (default: us-west-2) |
| `--help` | Show help message |

### `cleanup.sh`

| Option | Description |
|--------|-------------|
| `--region REGION` | AWS region (default: us-west-2) |
| `--stack-name NAME` | CloudFormation stack name |
| `--help` | Show help message |

## Troubleshooting

### Issue: Binary not found
**Error:**
```
[ERROR] Autopilot binary not found at: /path/to/target/release/iam-policy-autopilot
```

**Solution:**
```bash
cd iam-policy-autopilot
cargo build -p iam-policy-autopilot --release
```

### Issue: AWS credentials not configured
**Error:**
```
[ERROR] AWS credentials not configured or invalid
```

**Solution:**
```bash
aws configure
# OR
export AWS_PROFILE=your-profile
```

### Issue: Stack already exists
**Warning:**
```
[WARN] Stack iam-autopilot-integration-test already exists. Running pre-cleanup...
```

**Explanation:** The script automatically cleans up existing stacks. If this fails, run:
```bash
./run-integration-test.sh --cleanup
```

### Issue: IAM policy not propagated
**Error:**
```
[ERROR] Still seeing AccessDenied errors after fix
```

**Solution:** IAM policy propagation can take longer than 10 seconds. Try:
1. Run test with `--no-cleanup`
2. Wait 30 seconds
3. Re-invoke Lambda manually:
```bash
aws lambda invoke \
  --function-name IamAutopilotTestFunction \
  --region us-west-2 \
  /tmp/lambda-response.json
```

### Issue: CloudFormation stack deletion timeout
**Warning:**
```
[WARN] Stack deletion wait timed out or failed
```

**Solution:** Check stack status:
```bash
aws cloudformation describe-stacks \
  --stack-name iam-autopilot-integration-test \
  --region us-west-2
```

If stack is stuck, manually delete:
```bash
aws cloudformation delete-stack \
  --stack-name iam-autopilot-integration-test \
  --region us-west-2
```

## Test Architecture

### CloudFormation Resources
```
iam-autopilot-integration-test (Stack)
├── TestBucket (S3::Bucket)
│   └── iam-autopilot-test-{AccountId}
├── LambdaExecutionRole (IAM::Role)
│   ├── IamAutopilotTestLambdaRole
│   └── ManagedPolicy: AWSLambdaBasicExecutionRole
└── TestLambdaFunction (Lambda::Function)
    └── IamAutopilotTestFunction
```

### Lambda Function Logic
The Lambda function attempts three S3 operations:
1. `s3:GetObject` - Read a test object
2. `s3:PutObject` - Write a test object
3. `s3:ListBucket` - List bucket contents

All operations fail initially due to missing IAM permissions.

### IAM Policy Created by Autopilot
The autopilot creates an inline policy named `IamPolicyAutopilot-IamAutopilotTestLambdaRole` with statements like:
```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": ["s3:GetObject", "s3:PutObject"],
      "Resource": "arn:aws:s3:::iam-autopilot-test-123456789012/*"
    },
    {
      "Effect": "Allow",
      "Action": ["s3:ListBucket"],
      "Resource": "arn:aws:s3:::iam-autopilot-test-123456789012"
    }
  ]
}
```

## Running Tests in CI/CD

### GitHub Actions Example
```yaml
name: Integration Tests

on:
  push:
    branches: [main]
  pull_request:

jobs:
  integration-test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      
      - name: Configure AWS credentials
        uses: aws-actions/configure-aws-credentials@v2
        with:
          aws-access-key-id: ${{ secrets.AWS_ACCESS_KEY_ID }}
          aws-secret-access-key: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
          aws-region: us-west-2
      
      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      
      - name: Build autopilot
        run: cargo build -p iam-policy-autopilot --release
      
      - name: Run integration tests
        run: |
          cd iam-policy-autopilot/tests/integration
          ./run-integration-test.sh
```

### AWS CodeBuild Example
```yaml
version: 0.2

phases:
  install:
    runtime-versions:
      rust: 1.70
  
  build:
    commands:
      - cargo build -p iam-policy-autopilot --release
      - cd iam-policy-autopilot/tests/integration
      - ./run-integration-test.sh
```

## Cost Considerations

Running this test incurs minimal AWS costs:
- **CloudFormation:** Free
- **Lambda:** Free tier eligible (1M invocations/month)
- **S3:** Free tier eligible (5GB storage)
- **CloudWatch Logs:** Free tier eligible (5GB ingestion)
- **IAM:** Free

**Estimated cost per test run:** $0.00 (within free tier)

## Security Best Practices

1. **Use dedicated test account:** Run tests in a separate AWS account
2. **Limit IAM permissions:** Grant only necessary permissions
3. **Enable CloudTrail:** Monitor all API calls during testing
4. **Clean up resources:** Always run cleanup after tests
5. **Rotate credentials:** Rotate AWS credentials regularly

## Advanced Usage

### Testing in Multiple Regions
```bash
./run-integration-test.sh --region us-east-1
./run-integration-test.sh --region eu-west-1
./run-integration-test.sh --region ap-southeast-1
```

### Debugging Failed Tests
```bash
# Keep resources for manual inspection
./run-integration-test.sh --keep

# Manually inspect resources
aws cloudformation describe-stacks --stack-name iam-autopilot-integration-test
aws lambda get-function --function-name IamAutopilotTestFunction
aws iam get-role-policy --role-name IamAutopilotTestLambdaRole --policy-name IamPolicyAutopilot-IamAutopilotTestLambdaRole

# Clean up when done
./run-integration-test.sh --cleanup
```

### Custom Test Scenarios
You can modify `cfn-lambda-test.yaml` to test different scenarios:
- Add DynamoDB table and test DynamoDB permissions
- Add SQS queue and test queue permissions
- Add multiple Lambda functions with different roles

## Next Steps

After successful integration tests:
1. Review the IAM policy created by autopilot
2. Consider adding more test scenarios (DynamoDB, SQS, etc.)
3. Integrate tests into your CI/CD pipeline
4. Monitor test results and error patterns

## Support

For issues or questions:
- Check troubleshooting section above
- Review CloudWatch Logs for Lambda errors
- Check CloudFormation events for deployment issues
- Verify AWS credentials and permissions

## Related Documentation
- [IAM Policy Autopilot Complete Guide](../../AWS-IAM-POLICY-AUTOPILOT-COMPLETE-GUIDE.md)
- [Implementation Plan](../../INTEGRATION-TEST-IMPLEMENTATION-PLAN.md)
- [AWS CloudFormation Documentation](https://docs.aws.amazon.com/cloudformation/)
- [AWS Lambda Documentation](https://docs.aws.amazon.com/lambda/)
