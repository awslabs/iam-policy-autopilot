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
    """Configure logging for the application"""
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
    )
    return logging.getLogger(__name__)


# ── Data-plane helpers ─────────────────────────────────────────────────────────

def get_aws_account_id(sts_client):
    """Get the current AWS account ID using STS"""
    try:
        response = sts_client.get_caller_identity()
        return response['Account']
    except ClientError as e:
        raise Exception(f"Failed to get AWS account ID: {e}")


def upload_file_to_s3(s3_client, bucket_name, file_content, file_key):
    """Upload content to S3 bucket"""
    try:
        s3_client.put_object(
            Bucket=bucket_name,
            Key=file_key,
            Body=file_content,
            ContentType='application/json'
        )
        return True
    except ClientError as e:
        raise Exception(f"Failed to upload file to S3: {e}")


def send_sqs_message(sqs_client, queue_url, message_body):
    """Send a message to SQS queue"""
    try:
        response = sqs_client.send_message(
            QueueUrl=queue_url,
            MessageBody=json.dumps(message_body)
        )
        return response['MessageId']
    except ClientError as e:
        raise Exception(f"Failed to send SQS message: {e}")


def receive_sqs_messages(sqs_client, queue_url, max_messages=10):
    """Receive messages from SQS queue"""
    try:
        response = sqs_client.receive_message(
            QueueUrl=queue_url,
            MaxNumberOfMessages=max_messages,
            WaitTimeSeconds=5
        )
        return response.get('Messages', [])
    except ClientError as e:
        raise Exception(f"Failed to receive SQS messages: {e}")


def delete_sqs_message(sqs_client, queue_url, receipt_handle):
    """Delete a message from SQS queue"""
    try:
        sqs_client.delete_message(
            QueueUrl=queue_url,
            ReceiptHandle=receipt_handle
        )
        return True
    except ClientError as e:
        raise Exception(f"Failed to delete SQS message: {e}")


def put_cloudwatch_metric(cloudwatch_client, namespace, metric_name, value, unit='Count'):
    """Put a custom metric to CloudWatch"""
    try:
        cloudwatch_client.put_metric_data(
            Namespace=namespace,
            MetricData=[
                {
                    'MetricName': metric_name,
                    'Value': value,
                    'Unit': unit,
                    'Timestamp': datetime.utcnow()
                }
            ]
        )
        return True
    except ClientError as e:
        raise Exception(f"Failed to put CloudWatch metric: {e}")


# ── Main logic ─────────────────────────────────────────────────────────────────

def process_file_monitoring_system(bucket_name, queue_url, region, logger):
    """Main application logic - File Processing Monitoring System

    Assumes the S3 bucket and SQS queue already exist (created by CDK stack).
    """

    # Initialize AWS clients
    try:
        s3_client = boto3.client('s3', region_name=region)
        sqs_client = boto3.client('sqs', region_name=region)
        cloudwatch_client = boto3.client('cloudwatch', region_name=region)
        sts_client = boto3.client('sts', region_name=region)
    except NoCredentialsError:
        raise Exception("AWS credentials not found. Please configure your credentials.")

    # Get AWS account information using STS
    logger.info("Getting AWS account information...")
    account_id = get_aws_account_id(sts_client)
    logger.info(f"Using AWS Account ID: {account_id}")

    # Simulate file processing workflow
    files_to_process = [
        {"filename": "data1.json", "size": 1024, "type": "json"},
        {"filename": "data2.json", "size": 2048, "type": "json"},
        {"filename": "data3.json", "size": 512,  "type": "json"}
    ]

    processed_files = 0
    total_size = 0

    for file_info in files_to_process:
        # Create sample file content
        file_content = {
            "filename": file_info["filename"],
            "processed_at": datetime.utcnow().isoformat(),
            "size": file_info["size"],
            "type": file_info["type"],
            "processed_by": "file-monitoring-system"
        }

        # Upload file to S3
        logger.info(f"Uploading {file_info['filename']} to S3...")
        upload_file_to_s3(
            s3_client,
            bucket_name,
            json.dumps(file_content, indent=2),
            file_info["filename"]
        )

        # Send processing notification to SQS
        sqs_message = {
            "action": "file_processed",
            "filename": file_info["filename"],
            "bucket": bucket_name,
            "size": file_info["size"],
            "timestamp": datetime.utcnow().isoformat(),
            "account_id": account_id
        }

        logger.info("Sending processing notification to SQS...")
        message_id = send_sqs_message(sqs_client, queue_url, sqs_message)
        logger.info(f"SQS message sent with ID: {message_id}")

        # Update metrics
        processed_files += 1
        total_size += file_info["size"]

        # Send metrics to CloudWatch
        logger.info("Sending metrics to CloudWatch...")
        put_cloudwatch_metric(cloudwatch_client, "FileProcessing", "FilesProcessed", 1)
        put_cloudwatch_metric(cloudwatch_client, "FileProcessing", "BytesProcessed", file_info["size"], "Bytes")

        time.sleep(1)  # Small delay between processing

    # Process SQS messages (simulate monitoring)
    logger.info("Reading processing notifications from SQS...")
    messages = receive_sqs_messages(sqs_client, queue_url)

    for message in messages:
        message_body = json.loads(message['Body'])
        logger.info(f"Processing notification: {message_body['filename']} ({message_body['size']} bytes)")

        # Delete processed message
        delete_sqs_message(sqs_client, queue_url, message['ReceiptHandle'])
        logger.info("Notification processed and removed from queue")

    # Send final summary metrics to CloudWatch
    logger.info("Sending summary metrics to CloudWatch...")
    put_cloudwatch_metric(cloudwatch_client, "FileProcessing", "TotalFilesProcessed", processed_files)
    put_cloudwatch_metric(cloudwatch_client, "FileProcessing", "TotalBytesProcessed", total_size, "Bytes")

    logger.info("File processing monitoring completed!")
    logger.info(f"Total files processed: {processed_files}")
    logger.info(f"Total bytes processed: {total_size}")
    logger.info(f"S3 bucket: {bucket_name}")
    logger.info(f"SQS queue URL: {queue_url}")

    return {
        "bucket_name": bucket_name,
        "queue_url": queue_url,
        "processed_files": processed_files,
        "total_size": total_size
    }


# ── Entry point ────────────────────────────────────────────────────────────────

def main():
    logger = setup_logging()

    try:
        cfg = load_config()
    except FileNotFoundError as e:
        logger.error(str(e))
        sys.exit(1)

    bucket_name = cfg['bucketName']
    queue_url   = cfg['queueUrl']
    region      = cfg.get('region', 'us-east-1')

    try:
        logger.info("Starting AWS File Processing Monitoring System...")
        logger.info(f"Using bucket:    {bucket_name}")
        logger.info(f"Using queue URL: {queue_url}")
        logger.info(f"Using region:    {region}")

        result = process_file_monitoring_system(bucket_name, queue_url, region, logger)

        logger.info("=" * 60)
        logger.info("APPLICATION COMPLETED SUCCESSFULLY!")
        logger.info("=" * 60)
        logger.info("Resources used:")
        logger.info(f"  - S3 Bucket:          {result['bucket_name']}")
        logger.info(f"  - SQS Queue URL:      {result['queue_url']}")
        logger.info(f"  - CloudWatch Metrics: FileProcessing namespace")
        logger.info("Summary:")
        logger.info(f"  - Files processed:    {result['processed_files']}")
        logger.info(f"  - Total bytes:        {result['total_size']}")
        logger.info("=" * 60)
        logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")

    except Exception as e:
        logger.error(f"Application failed: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
