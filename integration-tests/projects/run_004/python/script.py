#!/usr/bin/env python3
import boto3, json, logging, os, sys
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
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(name)s - %(levelname)s - %(message)s',
    )
    return logging.getLogger(__name__)


def get_aws_account_id(sts_client):
    return sts_client.get_caller_identity()['Account']


def configure_xray_encryption(xray_client, logger):
    """Set X-Ray encryption to NONE (data-plane config call)."""
    xray_client.put_encryption_config(Type='NONE')
    logger.info("X-Ray encryption configured (Type=NONE)")


def run_ml_monitoring(cfg, logger):
    region = cfg.get('region', 'us-east-1')
    sts_client  = boto3.client('sts',  region_name=region)
    xray_client = boto3.client('xray', region_name=region)

    logger.info("Getting AWS account information...")
    account_id = get_aws_account_id(sts_client)
    logger.info(f"AWS Account ID: {account_id}")

    logger.info("Configuring X-Ray encryption...")
    configure_xray_encryption(xray_client, logger)

    return {
        'account_id':          account_id,
        'cluster_name':        cfg['clusterName'],
        'log_group_name':      cfg['logGroupName'],
        'kms_key_id':          cfg['kmsKeyId'],
        'resource_group_name': cfg['resourceGroupName'],
        'region':              region,
    }


def main():
    logger = setup_logging()
    try:
        cfg = load_config()
    except FileNotFoundError as e:
        logger.error(str(e))
        sys.exit(1)

    logger.info("Starting ML Monitoring Platform...")
    logger.info(f"Using ECS cluster:    {cfg['clusterName']}")
    logger.info(f"Using log group:      {cfg['logGroupName']}")
    logger.info(f"Using KMS key:        {cfg['kmsKeyId']}")
    logger.info(f"Using resource group: {cfg['resourceGroupName']}")
    logger.info(f"Using region:         {cfg.get('region', 'us-east-1')}")

    try:
        result = run_ml_monitoring(cfg, logger)
        logger.info("=" * 60)
        logger.info("APPLICATION COMPLETED SUCCESSFULLY!")
        logger.info("=" * 60)
        logger.info("Resources used:")
        logger.info(f"  - ECS Cluster:    {result['cluster_name']}")
        logger.info(f"  - Log Group:      {result['log_group_name']}")
        logger.info(f"  - KMS Key:        {result['kms_key_id']}")
        logger.info(f"  - Resource Group: {result['resource_group_name']}")
        logger.info("=" * 60)
        logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")
    except (ClientError, NoCredentialsError) as e:
        logger.error(f"AWS error: {e}")
        sys.exit(1)
    except Exception as e:
        logger.error(f"Application failed: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
