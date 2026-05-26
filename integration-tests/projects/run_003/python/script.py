#!/usr/bin/env python3
import boto3
import logging
import sys
import time
import json
import os
from datetime import datetime
from botocore.exceptions import ClientError, NoCredentialsError

# ── Config loading ─────────────────────────────────────────────────────────────

def load_config():
    """Load infrastructure config written by deploy.sh."""
    config_path = os.path.join(os.path.dirname(__file__), '..', 'config.json')
    config_path = os.path.normpath(config_path)
    if not os.path.exists(config_path):
        raise FileNotFoundError(
            f"config.json not found at {config_path}.\n"
            "Deploy the CDK stack first:\n"
            "  cd ../cdk && bash deploy.sh"
        )
    with open(config_path) as f:
        return json.load(f)


# ── Logging ────────────────────────────────────────────────────────────────────

def setup_logging():
    """Configure logging for the application."""
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
    )
    return logging.getLogger(__name__)


# ── Data-plane helpers ─────────────────────────────────────────────────────────

def get_aws_account_id(sts_client):
    """Get the current AWS account ID using STS."""
    try:
        response = sts_client.get_caller_identity()
        return response['Account']
    except ClientError as e:
        raise Exception(f"Failed to get AWS account ID: {e}")


def upload_sample_data(s3_client, bucket_name, kms_key_id, timestamp, logger):
    """Upload sample data to S3 with KMS encryption."""
    key = f"data/sample-{timestamp}.json"
    content = json.dumps({
        "timestamp": timestamp,
        "data": "Sample data for processing pipeline",
        "processed": False
    })
    try:
        s3_client.put_object(
            Bucket=bucket_name,
            Key=key,
            Body=content,
            ContentType='application/json',
            ServerSideEncryption='aws:kms',
            SSEKMSKeyId=kms_key_id
        )
        logger.info(f"Uploaded sample data to s3://{bucket_name}/{key}")
        return key
    except ClientError as e:
        raise Exception(f"Failed to upload sample data to S3: {e}")


def start_pipeline_execution(sfn_client, state_machine_arn, bucket_name, timestamp, logger):
    """Start a Step Functions execution."""
    execution_input = json.dumps({
        "bucket": bucket_name,
        "timestamp": timestamp
    })
    try:
        response = sfn_client.start_execution(
            stateMachineArn=state_machine_arn,
            input=execution_input
        )
        execution_arn = response['executionArn']
        logger.info(f"Started execution: {execution_arn}")
        return execution_arn
    except ClientError as e:
        raise Exception(f"Failed to start Step Functions execution: {e}")


def poll_execution(sfn_client, execution_arn, timeout_seconds, logger):
    """Poll Step Functions execution until terminal state or timeout."""
    terminal_statuses = {'SUCCEEDED', 'FAILED', 'TIMED_OUT', 'ABORTED'}
    deadline = time.time() + timeout_seconds
    while time.time() < deadline:
        response = sfn_client.describe_execution(executionArn=execution_arn)
        status = response['status']
        logger.info(f"Execution status: {status}")
        if status in terminal_statuses:
            return status
        time.sleep(5)
    raise Exception(f"Execution did not reach terminal state within {timeout_seconds}s")


def put_pipeline_metrics(cloudwatch_client, logger):
    """Put custom CloudWatch metrics for the pipeline run."""
    namespace = 'DataProcessingPipeline'
    metrics = [
        {'MetricName': 'PipelineExecutions', 'Value': 1, 'Unit': 'Count'},
        {'MetricName': 'FilesProcessed',     'Value': 1, 'Unit': 'Count'},
    ]
    try:
        cloudwatch_client.put_metric_data(
            Namespace=namespace,
            MetricData=[
                {
                    'MetricName': m['MetricName'],
                    'Value': m['Value'],
                    'Unit': m['Unit'],
                    'Timestamp': datetime.utcnow()
                }
                for m in metrics
            ]
        )
        logger.info(f"Published metrics to CloudWatch namespace '{namespace}'")
    except ClientError as e:
        raise Exception(f"Failed to put CloudWatch metrics: {e}")


# ── Main logic ─────────────────────────────────────────────────────────────────

def run_data_pipeline(cfg, logger):
    """Main data-plane logic — assumes all infrastructure already exists."""
    bucket_name      = cfg['bucketName']
    kms_key_id       = cfg['kmsKeyId']
    state_machine_arn = cfg['stateMachineArn']
    region           = cfg.get('region', 'us-east-1')

    try:
        s3_client         = boto3.client('s3',         region_name=region)
        sfn_client        = boto3.client('stepfunctions', region_name=region)
        cloudwatch_client = boto3.client('cloudwatch', region_name=region)
        sts_client        = boto3.client('sts',        region_name=region)
    except NoCredentialsError:
        raise Exception("AWS credentials not found. Please configure your credentials.")

    # 1. Get account ID
    logger.info("Getting AWS account information...")
    account_id = get_aws_account_id(sts_client)
    logger.info(f"AWS Account ID: {account_id}")

    # 2. Upload sample data with KMS encryption
    timestamp = int(time.time())
    logger.info("Uploading sample data to S3 with KMS encryption...")
    data_key = upload_sample_data(s3_client, bucket_name, kms_key_id, timestamp, logger)

    # 3. Start Step Functions execution
    logger.info("Starting Step Functions pipeline execution...")
    execution_arn = start_pipeline_execution(
        sfn_client, state_machine_arn, bucket_name, timestamp, logger
    )

    # 4. Poll for completion (60s timeout, 5s interval)
    logger.info("Polling for execution completion (timeout: 60s)...")
    final_status = poll_execution(sfn_client, execution_arn, 60, logger)
    logger.info(f"Execution finished with status: {final_status}")

    # 5. Put custom CloudWatch metrics
    logger.info("Publishing custom CloudWatch metrics...")
    put_pipeline_metrics(cloudwatch_client, logger)

    return {
        'account_id':      account_id,
        'bucket_name':     bucket_name,
        'data_key':        data_key,
        'execution_arn':   execution_arn,
        'execution_status': final_status,
        'state_machine_arn': state_machine_arn,
        'region':          region,
    }


# ── Entry point ────────────────────────────────────────────────────────────────

def main():
    logger = setup_logging()

    try:
        cfg = load_config()
    except FileNotFoundError as e:
        logger.error(str(e))
        sys.exit(1)

    logger.info("Starting AWS Data Processing Pipeline...")
    logger.info(f"Using bucket:        {cfg['bucketName']}")
    logger.info(f"Using KMS key:       {cfg['kmsKeyId']}")
    logger.info(f"Using state machine: {cfg['stateMachineArn']}")
    logger.info(f"Using region:        {cfg.get('region', 'us-east-1')}")

    try:
        result = run_data_pipeline(cfg, logger)

        logger.info("=" * 60)
        logger.info("APPLICATION COMPLETED SUCCESSFULLY!")
        logger.info("=" * 60)
        logger.info("Resources used:")
        logger.info(f"  - S3 Bucket:          {result['bucket_name']}")
        logger.info(f"  - Data key:           {result['data_key']}")
        logger.info(f"  - State Machine:      {result['state_machine_arn']}")
        logger.info(f"  - CloudWatch Metrics: DataProcessingPipeline namespace")
        logger.info("Summary:")
        logger.info(f"  - Execution ARN:      {result['execution_arn']}")
        logger.info(f"  - Execution status:   {result['execution_status']}")
        logger.info("=" * 60)
        logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")

    except Exception as e:
        logger.error(f"Application failed: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
