#!/usr/bin/env python3
import boto3
import logging
import sys
import json
import os
import time
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


def execute_redshift_statement(redshift_data_client, cluster_id, database, db_user, sql, logger):
    """Execute a SQL statement via the Redshift Data API and return the execution ID."""
    try:
        response = redshift_data_client.execute_statement(
            ClusterIdentifier=cluster_id,
            Database=database,
            DbUser=db_user,
            Sql=sql,
        )
        statement_id = response['Id']
        logger.info(f"Redshift Data API ExecuteStatement submitted, id={statement_id}")
        return statement_id
    except ClientError as e:
        raise Exception(f"Failed to execute Redshift statement: {e}")


def wait_for_redshift_statement(redshift_data_client, statement_id, logger, poll_interval=2, max_wait=60):
    """Poll DescribeStatement until the statement finishes (or times out)."""
    elapsed = 0
    while elapsed < max_wait:
        response = redshift_data_client.describe_statement(Id=statement_id)
        status = response['Status']
        logger.info(f"  Statement {statement_id} status: {status}")
        if status in ('FINISHED', 'FAILED', 'ABORTED'):
            if status != 'FINISHED':
                error = response.get('Error', 'unknown error')
                logger.warning(f"  Statement ended with status {status}: {error}")
            return status
        time.sleep(poll_interval)
        elapsed += poll_interval
    logger.warning(f"  Statement {statement_id} did not finish within {max_wait}s")
    return 'TIMEOUT'


# ── Main logic ─────────────────────────────────────────────────────────────────

def run_security_analytics(cluster_id, region, logger):
    """
    Main application logic — Security and Analytics Platform data-plane.

    Assumes all infrastructure already exists (created by CDK stack).
    Runs 3 Redshift Data API ExecuteStatement calls:
      1. CREATE TABLE (security_events)
      2. INSERT data
      3. Analytics SELECT query
    """

    try:
        sts_client           = boto3.client('sts',           region_name=region)
        redshift_data_client = boto3.client('redshift-data', region_name=region)
    except NoCredentialsError:
        raise Exception("AWS credentials not found. Please configure your credentials.")

    # ── STS: GetCallerIdentity ─────────────────────────────────────────────────
    logger.info("Getting AWS account information...")
    account_id = get_aws_account_id(sts_client)
    logger.info(f"Using AWS Account ID: {account_id}")

    database = 'securitydb'
    db_user  = 'adminuser'

    # ── Redshift Data API: 1. CREATE TABLE ────────────────────────────────────
    logger.info("Executing Redshift statement 1/3: CREATE TABLE security_events...")
    create_sql = """
        CREATE TABLE IF NOT EXISTS security_events (
            event_id    VARCHAR(64),
            event_type  VARCHAR(64),
            source_ip   VARCHAR(45),
            user_name   VARCHAR(128),
            timestamp   TIMESTAMP,
            severity    VARCHAR(16),
            description VARCHAR(512)
        )
    """.strip()

    stmt_id = execute_redshift_statement(
        redshift_data_client, cluster_id, database, db_user, create_sql, logger
    )
    wait_for_redshift_statement(redshift_data_client, stmt_id, logger)

    # ── Redshift Data API: 2. INSERT data ─────────────────────────────────────
    logger.info("Executing Redshift statement 2/3: INSERT security events...")
    insert_sql = """
        INSERT INTO security_events
            (event_id, event_type, source_ip, user_name, timestamp, severity, description)
        VALUES
            ('evt-001', 'LOGIN_FAILURE',  '192.168.1.100', 'user1',  GETDATE(), 'HIGH',   'Multiple failed login attempts'),
            ('evt-002', 'DATA_ACCESS',    '10.0.0.50',     'user2',  GETDATE(), 'MEDIUM', 'Unusual data access pattern'),
            ('evt-003', 'PRIVILEGE_ESCALATION', '172.16.0.1', 'user3', GETDATE(), 'CRITICAL', 'Unauthorized privilege escalation attempt')
    """.strip()

    stmt_id = execute_redshift_statement(
        redshift_data_client, cluster_id, database, db_user, insert_sql, logger
    )
    wait_for_redshift_statement(redshift_data_client, stmt_id, logger)

    # ── Redshift Data API: 3. Analytics SELECT ────────────────────────────────
    logger.info("Executing Redshift statement 3/3: Analytics query on security_events...")
    analytics_sql = """
        SELECT
            severity,
            COUNT(*)          AS event_count,
            MIN(timestamp)    AS first_seen,
            MAX(timestamp)    AS last_seen
        FROM security_events
        GROUP BY severity
        ORDER BY event_count DESC
    """.strip()

    stmt_id = execute_redshift_statement(
        redshift_data_client, cluster_id, database, db_user, analytics_sql, logger
    )
    wait_for_redshift_statement(redshift_data_client, stmt_id, logger)

    return {
        'account_id': account_id,
        'cluster_id': cluster_id,
        'region':     region,
    }


# ── Entry point ────────────────────────────────────────────────────────────────

def main():
    logger = setup_logging()

    try:
        cfg = load_config()
    except FileNotFoundError as e:
        logger.error(str(e))
        sys.exit(1)

    cluster_id = cfg['redshiftClusterIdentifier']
    region     = cfg.get('region', 'us-east-1')

    try:
        logger.info("Starting AWS Security and Analytics Platform (data-plane)...")
        logger.info(f"Using Redshift cluster: {cluster_id}")
        logger.info(f"Using region:           {region}")

        result = run_security_analytics(cluster_id, region, logger)

        logger.info("=" * 60)
        logger.info("APPLICATION COMPLETED SUCCESSFULLY!")
        logger.info("=" * 60)
        logger.info("Resources used (data-plane):")
        logger.info(f"  - STS:           GetCallerIdentity (account: {result['account_id']})")
        logger.info(f"  - Redshift Data: ExecuteStatement x3 (cluster: {result['cluster_id']})")
        logger.info("=" * 60)
        logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")

    except Exception as e:
        logger.error(f"Application failed: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
