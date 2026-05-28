#!/usr/bin/env python3

import json
import logging
import os
import random
import sys
import time
from pathlib import Path

import boto3
from botocore.exceptions import ClientError, NoCredentialsError


def setup_logging():
    """Setup logging configuration"""
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(levelname)s - %(message)s'
    )
    return logging.getLogger(__name__)


def load_config():
    """Load config.json from one directory above this script."""
    script_dir = Path(__file__).resolve().parent
    config_path = script_dir.parent / 'config.json'
    if not config_path.exists():
        raise RuntimeError(
            f"config.json not found at {config_path}.\n"
            "Deploy the CDK stack first:\n"
            "  cd ../cdk && bash deploy.sh"
        )
    with open(config_path) as f:
        return json.load(f)


def upload_sample_data(s3_client, bucket_name, logger):
    """Upload sample monitoring data to S3"""
    try:
        sample_data = {
            "timestamp": time.time(),
            "system_metrics": {
                "cpu_usage": random.uniform(10, 90),
                "memory_usage": random.uniform(20, 80),
                "disk_usage": random.uniform(30, 70)
            },
            "application_metrics": {
                "requests_per_second": random.randint(100, 1000),
                "error_rate": random.uniform(0.1, 5.0),
                "response_time": random.uniform(100, 500)
            }
        }

        s3_client.put_object(
            Bucket=bucket_name,
            Key=f'monitoring-data/{int(time.time())}.json',
            Body=json.dumps(sample_data, indent=2),
            ContentType='application/json'
        )
        logger.info("Uploaded sample monitoring data to S3")
        return True
    except ClientError as e:
        logger.error(f"Failed to upload sample data: {e}")
        return False


def send_notification_email(ses_client, sender_email, recipient_email, logger):
    """Send notification email using SES.

    Only MessageRejected is tolerated (unverified addresses in test/sandbox
    environments).  All other errors — including AccessDeniedException —
    propagate so the minimizer can detect missing permissions.
    """
    try:
        subject = "AWS Monitoring System - Setup Complete"
        body = (
            "Your AWS monitoring system has been successfully set up.\n\n"
            "The system is now ready for use."
        )

        ses_client.send_email(
            Source=sender_email,
            Destination={'ToAddresses': [recipient_email]},
            Message={
                'Subject': {'Data': subject},
                'Body': {'Text': {'Data': body}}
            }
        )
        logger.info(f"Sent notification email to {recipient_email}")
    except ClientError as e:
        if e.response['Error']['Code'] == 'MessageRejected':
            logger.warning("SES SendEmail: email not verified (non-fatal)")
        else:
            raise  # Re-raise AccessDeniedException and other errors


def main():
    logger = setup_logging()

    try:
        cfg = load_config()
    except RuntimeError as e:
        logger.error(str(e))
        sys.exit(1)

    bucket_name = cfg['bucketName']
    region = cfg.get('region', 'us-east-1')

    logger.info("Starting AWS Comprehensive Monitoring System (data-plane)...")
    logger.info(f"Using bucket: {bucket_name}")
    logger.info(f"Using region: {region}")

    try:
        session = boto3.Session(region_name=region)
        sts_client = session.client('sts')
        s3_client = session.client('s3')
        ses_client = session.client('ses')

        # STS GetCallerIdentity — verify credentials
        identity = sts_client.get_caller_identity()
        logger.info(f"AWS Account ID: {identity['Account']}")

        # S3 PutObject — upload monitoring data
        if not upload_sample_data(s3_client, bucket_name, logger):
            raise Exception("Failed to upload sample data to S3")

        # SES SendEmail — MessageRejected is tolerated; permission errors propagate
        send_notification_email(
            ses_client,
            sender_email='test@example.com',
            recipient_email='test@example.com',
            logger=logger
        )

        logger.info("AWS Comprehensive Monitoring System completed successfully!")
        logger.info(f"  - S3 Bucket: {bucket_name}")

    except NoCredentialsError:
        logger.error("AWS credentials not found. Please configure your credentials.")
        sys.exit(1)
    except ClientError as e:
        logger.error(f"AWS service error: {e}")
        sys.exit(1)
    except Exception as e:
        logger.error(f"Unexpected error: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
