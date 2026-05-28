#!/usr/bin/env python3
import boto3, json, logging, os, sys
from botocore.exceptions import ClientError, NoCredentialsError

def load_config():
    config_path = os.path.normpath(os.path.join(os.path.dirname(__file__), '..', 'config.json'))
    if not os.path.exists(config_path):
        raise FileNotFoundError(f"config.json not found at {config_path}.\nDeploy the CDK stack first:\n  cd ../cdk && bash deploy.sh")
    with open(config_path) as f:
        return json.load(f)

def setup_logging():
    logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(name)s - %(levelname)s - %(message)s')
    return logging.getLogger(__name__)

def get_aws_account_id(sts_client):
    return sts_client.get_caller_identity()['Account']

def retrieve_secret(secrets_client, secret_name, logger):
    """Retrieve and verify the stored secret from Secrets Manager."""
    response = secrets_client.get_secret_value(SecretId=secret_name)
    secret_data = json.loads(response['SecretString'])
    logger.info("Successfully retrieved and decrypted configuration from Secrets Manager")
    return secret_data

def send_notification(sns_client, topic_arn, repo_name, clone_url, logger):
    """Send a notification about the repository via SNS."""
    message = {
        "default": f"Secure repository '{repo_name}' is configured and ready.",
        "email": f"Repository Monitoring Alert\n\nRepository: {repo_name}\nClone URL: {clone_url}\n\nSecurity features: KMS encryption, SNS notifications, Secrets Manager integration."
    }
    sns_client.publish(
        TopicArn=topic_arn,
        Message=json.dumps(message),
        MessageStructure='json',
        Subject=f"Repository Ready: {repo_name}"
    )
    logger.info("Notification sent successfully")

def run_secure_repo_monitoring(cfg, logger):
    region = cfg.get('region', 'us-east-1')
    sts_client     = boto3.client('sts',            region_name=region)
    secrets_client = boto3.client('secretsmanager', region_name=region)
    sns_client     = boto3.client('sns',            region_name=region)

    logger.info("Getting AWS account information...")
    account_id = get_aws_account_id(sts_client)
    logger.info(f"AWS Account ID: {account_id}")

    logger.info("Retrieving configuration from Secrets Manager...")
    secret_data = retrieve_secret(secrets_client, cfg['secretName'], logger)
    logger.info(f"Verified configuration for repository: {secret_data.get('repository_name', cfg['repoName'])}")

    logger.info("Sending repository notification via SNS...")
    send_notification(sns_client, cfg['topicArn'], cfg['repoName'], cfg['cloneUrl'], logger)

    return {
        'account_id': account_id,
        'topic_arn':  cfg['topicArn'],
        'secret_name': cfg['secretName'],
        'repo_name':  cfg['repoName'],
        'clone_url':  cfg['cloneUrl'],
        'region':     region,
    }

def main():
    logger = setup_logging()
    try:
        cfg = load_config()
    except FileNotFoundError as e:
        logger.error(str(e))
        sys.exit(1)

    logger.info("Starting Secure Repository Monitoring...")
    logger.info(f"Using SNS topic:  {cfg['topicArn']}")
    logger.info(f"Using secret:     {cfg['secretName']}")
    logger.info(f"Using repo:       {cfg['repoName']}")
    logger.info(f"Using region:     {cfg.get('region', 'us-east-1')}")

    try:
        result = run_secure_repo_monitoring(cfg, logger)
        logger.info("=" * 60)
        logger.info("APPLICATION COMPLETED SUCCESSFULLY!")
        logger.info("=" * 60)
        logger.info("Resources used:")
        logger.info(f"  - SNS Topic:  {result['topic_arn']}")
        logger.info(f"  - Secret:     {result['secret_name']}")
        logger.info(f"  - Repo:       {result['repo_name']}")
        logger.info(f"  - Clone URL:  {result['clone_url']}")
        logger.info("=" * 60)
        logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")
    except Exception as e:
        logger.error(f"Application failed: {e}")
        sys.exit(1)

if __name__ == "__main__":
    main()
