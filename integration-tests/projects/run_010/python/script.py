#!/usr/bin/env python3
import boto3
import json
import logging
import os
import sys
from botocore.exceptions import ClientError, NoCredentialsError


def load_config():
    config_path = os.path.normpath(os.path.join(os.path.dirname(__file__), '..', 'config.json'))
    if not os.path.exists(config_path):
        raise FileNotFoundError(
            f"config.json not found at {config_path}.\n"
            "Deploy the CDK stack first:\n"
            "  cd ../cdk && bash deploy.sh"
        )
    with open(config_path) as f:
        return json.load(f)


def setup_logging():
    logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(name)s - %(levelname)s - %(message)s')
    return logging.getLogger(__name__)


def main():
    logger = setup_logging()

    try:
        cfg = load_config()
    except FileNotFoundError as e:
        logger.error(str(e))
        sys.exit(1)

    region = cfg.get('region', 'us-east-1')
    function_name = cfg['functionName']
    log_group_name = cfg['logGroupName']

    logger.info("Starting AWS Deployment Monitoring...")
    logger.info(f"Using function:   {function_name}")
    logger.info(f"Using log group:  {log_group_name}")
    logger.info(f"Using region:     {region}")

    try:
        # Test credentials via STS
        sts_client = boto3.client('sts', region_name=region)
        identity = sts_client.get_caller_identity()
        logger.info(f"Running as: {identity.get('Arn', 'Unknown')}")

        # Invoke the Lambda function
        lambda_client = boto3.client('lambda', region_name=region)
        logger.info(f"Invoking Lambda function: {function_name}")
        response = lambda_client.invoke(
            FunctionName=function_name,
            InvocationType='RequestResponse'
        )

        status_code = response['StatusCode']
        payload = json.loads(response['Payload'].read().decode())
        logger.info(f"Lambda invocation status: {status_code}")
        logger.info(f"Lambda response: {payload}")

        if status_code != 200:
            raise RuntimeError(f"Lambda invocation returned unexpected status: {status_code}")

        logger.info("=" * 60)
        logger.info("APPLICATION COMPLETED SUCCESSFULLY!")
        logger.info("=" * 60)
        logger.info("Resources used:")
        logger.info(f"  - Lambda Function: {function_name}")
        logger.info(f"  - Log Group:       {log_group_name}")
        logger.info("=" * 60)
        logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")

    except NoCredentialsError:
        logger.error("AWS credentials not found. Please configure your credentials.")
        sys.exit(1)
    except Exception as e:
        logger.error(f"Application failed: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
