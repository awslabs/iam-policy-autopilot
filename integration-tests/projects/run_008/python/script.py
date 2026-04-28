#!/usr/bin/env python3
import boto3
import json
import logging
import sys
import time
from pathlib import Path
from botocore.exceptions import ClientError


def load_config():
    """Load config.json from ../config.json relative to this script."""
    config_path = Path(__file__).parent.parent / "config.json"
    if not config_path.exists():
        raise RuntimeError(
            f"config.json not found at {config_path}.\n"
            "Deploy the CDK stack first:\n"
            "  cd ../cdk && bash deploy.sh"
        )
    with open(config_path) as f:
        return json.load(f)


def log_to_cloudwatch(logs_client, log_group_name, log_stream_name, message):
    """Create a log stream and send a log message to CloudWatch."""
    # Create log stream (ignore if already exists)
    try:
        logs_client.create_log_stream(
            logGroupName=log_group_name,
            logStreamName=log_stream_name
        )
    except ClientError as e:
        if e.response['Error']['Code'] != 'ResourceAlreadyExistsException':
            raise

    log_event = {
        'timestamp': int(time.time() * 1000),
        'message': message
    }

    logs_client.put_log_events(
        logGroupName=log_group_name,
        logStreamName=log_stream_name,
        logEvents=[log_event]
    )
    logging.info("Logged to CloudWatch: %s", message)


def list_portfolios_and_products(sc_client, logs_client, log_group_name, log_stream_name):
    """List all portfolios and search products within each portfolio."""
    portfolios_response = sc_client.list_portfolios()
    portfolio_details = portfolios_response.get('PortfolioDetails', [])

    portfolio_info = []
    for portfolio in portfolio_details:
        portfolio_id = portfolio['Id']
        portfolio_name = portfolio['DisplayName']

        product_list = []
        try:
            products_response = sc_client.search_products_as_admin(
                PortfolioId=portfolio_id
            )
            for product in products_response.get('ProductViewDetails', []):
                product_list.append({
                    'Id': product['ProductViewSummary']['ProductId'],
                    'Name': product['ProductViewSummary']['Name']
                })
        except Exception as e:
            logging.warning("Failed to get products for portfolio %s: %s", portfolio_id, e)

        portfolio_info.append({
            'Id': portfolio_id,
            'Name': portfolio_name,
            'Products': product_list
        })

    info_msg = f"Found {len(portfolio_info)} portfolios"
    logging.info(info_msg)
    log_to_cloudwatch(logs_client, log_group_name, log_stream_name, info_msg)

    for portfolio in portfolio_info:
        detail_msg = (
            f"Portfolio: {portfolio['Name']} ({portfolio['Id']}) "
            f"has {len(portfolio['Products'])} products"
        )
        logging.info(detail_msg)
        log_to_cloudwatch(logs_client, log_group_name, log_stream_name, detail_msg)

    return portfolio_info


def main():
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(levelname)s - %(message)s'
    )
    logger = logging.getLogger(__name__)

    cfg = load_config()
    log_group_name = cfg['logGroupName']
    region = cfg.get('region', 'us-east-1')

    logger.info("Starting AWS Service Catalog Manager")
    logger.info("Log group: %s", log_group_name)
    logger.info("Region:    %s", region)

    sts_client = boto3.client('sts', region_name=region)
    logs_client = boto3.client('logs', region_name=region)
    sc_client = boto3.client('servicecatalog', region_name=region)

    # Verify credentials
    identity = sts_client.get_caller_identity()
    logger.info("AWS Account ID: %s", identity['Account'])

    log_stream_name = f"service-catalog-manager-{int(time.time())}"

    # Log startup
    log_to_cloudwatch(logs_client, log_group_name, log_stream_name,
                      "Service Catalog Manager started")

    # List portfolios and products
    logger.info("Listing portfolios and products...")
    portfolio_info = list_portfolios_and_products(
        sc_client, logs_client, log_group_name, log_stream_name
    )

    # Log completion
    completion_msg = "Service Catalog Manager completed successfully"
    log_to_cloudwatch(logs_client, log_group_name, log_stream_name, completion_msg)

    logger.info("=" * 60)
    logger.info("SERVICE CATALOG MANAGER COMPLETED")
    logger.info("=" * 60)
    logger.info("Region:           %s", region)
    logger.info("Log Group:        %s", log_group_name)
    logger.info("Log Stream:       %s", log_stream_name)
    logger.info("Portfolios found: %d", len(portfolio_info))
    logger.info("=" * 60)


if __name__ == "__main__":
    main()
