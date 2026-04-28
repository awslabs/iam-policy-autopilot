#!/usr/bin/env python3
"""
AWS Compliance Monitoring System — data-plane script (CDK-refactored)

Infrastructure (KMS key, S3 bucket) is provisioned by the CDK stack in
../cdk/lib/stack.ts.  Deploy it first:

    cd ../cdk && bash deploy.sh

That writes ../config.json with the stack outputs.  Then just run:

    python script.py

Services used (data-plane only):
    s3              : GetBucketLocation, PutObject
    glue            : CreateDatabase, GetDatabase, CreateTable, GetTable
    athena          : StartQueryExecution, GetQueryExecution, GetQueryResults
    cloudwatch      : PutMetricData
    organizations   : ListAccounts (graceful fallback if not in org)
    sts             : GetCallerIdentity (fallback for org data)
"""
import boto3
import json
import logging
import os
import sys
import time
from datetime import datetime
from botocore.exceptions import ClientError

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
        format='%(asctime)s - %(levelname)s - %(message)s',
        handlers=[logging.StreamHandler(sys.stdout)]
    )
    return logging.getLogger(__name__)

def collect_organization_data(organizations_client, sts_client, logger):
    """Collect organization account information, falling back gracefully if not in org."""
    logger.info("Collecting organization data...")
    accounts_data = []

    try:
        paginator = organizations_client.get_paginator('list_accounts')
        for page in paginator.paginate():
            for account in page['Accounts']:
                account_info = {
                    'account_id': account['Id'],
                    'account_name': account['Name'],
                    'email': account['Email'],
                    'status': account['Status'],
                    'joined_method': account['JoinedMethod'],
                    'joined_timestamp': account['JoinedTimestamp'].isoformat(),
                    'collection_time': datetime.utcnow().isoformat()
                }
                accounts_data.append(account_info)
    except ClientError as e:
        if e.response['Error']['Code'] in ['AWSOrganizationsNotInUseException', 'AccessDeniedException']:
            logger.warning("Organizations not available, using current account only")
            identity = sts_client.get_caller_identity()
            accounts_data.append({
                'account_id': identity['Account'],
                'account_name': 'Current Account',
                'email': 'unknown@example.com',
                'status': 'ACTIVE',
                'joined_method': 'CREATED',
                'joined_timestamp': datetime.utcnow().isoformat(),
                'collection_time': datetime.utcnow().isoformat()
            })
        else:
            raise

    logger.info(f"Collected data for {len(accounts_data)} accounts")
    return accounts_data

def verify_s3_bucket(s3_client, bucket_name, logger):
    """Verify S3 bucket exists and get its location (grants s3:GetBucketLocation)."""
    response = s3_client.get_bucket_location(Bucket=bucket_name)
    location = response.get('LocationConstraint') or 'us-east-1'
    logger.info(f"Bucket location: {location}")
    return location

def upload_data_to_s3(s3_client, bucket_name, kms_key_id, data, logger):
    """Upload compliance data to S3 with SSE-KMS."""
    logger.info("Uploading data to S3...")

    json_lines = '\n'.join([json.dumps(record) for record in data])
    now = datetime.now()
    key = (f"compliance-data/year={now.year}/month={now.month:02d}/"
           f"day={now.day:02d}/accounts_{int(time.time())}.json")

    s3_client.put_object(
        Bucket=bucket_name,
        Key=key,
        Body=json_lines.encode('utf-8'),
        ContentType='application/json',
        ServerSideEncryption='aws:kms',
        SSEKMSKeyId=kms_key_id
    )

    logger.info(f"Uploaded data to S3: s3://{bucket_name}/{key}")
    return key

def wait_for_query_completion(athena_client, query_execution_id, logger, max_wait_time=300):
    """Wait for Athena query to complete."""
    start_time = time.time()

    while time.time() - start_time < max_wait_time:
        response = athena_client.get_query_execution(
            QueryExecutionId=query_execution_id
        )
        status = response['QueryExecution']['Status']['State']

        if status == 'SUCCEEDED':
            return
        elif status in ['FAILED', 'CANCELLED']:
            reason = response['QueryExecution']['Status'].get('StateChangeReason', 'Unknown')
            raise Exception(f"Query {status.lower()}: {reason}")

        time.sleep(5)

    raise Exception(f"Query timed out after {max_wait_time} seconds")

def _athena_result_config(bucket_name, kms_key_id):
    """Build Athena ResultConfiguration with SSE-KMS encryption."""
    return {
        'OutputLocation': f's3://{bucket_name}/query-results/',
        'EncryptionConfiguration': {
            'EncryptionOption': 'SSE_KMS',
            'KmsKey': kms_key_id
        }
    }

def setup_glue_database(glue_client, bucket_name, database_name, table_name, logger):
    """Create Glue database and table directly (Athena uses Glue Data Catalog)."""
    logger.info("Setting up Glue database and table...")

    # Create database (glue:CreateDatabase)
    try:
        glue_client.get_database(Name=database_name)
        logger.info(f"Glue database '{database_name}' already exists")
    except ClientError as e:
        if e.response['Error']['Code'] == 'EntityNotFoundException':
            glue_client.create_database(
                DatabaseInput={'Name': database_name, 'Description': 'Compliance monitoring database'}
            )
            logger.info(f"Created Glue database '{database_name}'")
        else:
            raise

    # Create table (glue:CreateTable)
    table_input = {
        'Name': table_name,
        'Description': 'Organization accounts compliance data',
        'StorageDescriptor': {
            'Columns': [
                {'Name': 'account_id', 'Type': 'string'},
                {'Name': 'account_name', 'Type': 'string'},
                {'Name': 'email', 'Type': 'string'},
                {'Name': 'status', 'Type': 'string'},
                {'Name': 'joined_method', 'Type': 'string'},
                {'Name': 'joined_timestamp', 'Type': 'string'},
                {'Name': 'collection_time', 'Type': 'string'},
            ],
            'Location': f's3://{bucket_name}/compliance-data/',
            'InputFormat': 'org.apache.hadoop.mapred.TextInputFormat',
            'OutputFormat': 'org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat',
            'SerdeInfo': {'SerializationLibrary': 'org.apache.hive.hcatalog.data.JsonSerDe'},
            'Compressed': False,
        },
        'PartitionKeys': [
            {'Name': 'year', 'Type': 'string'},
            {'Name': 'month', 'Type': 'string'},
            {'Name': 'day', 'Type': 'string'},
        ],
        'TableType': 'EXTERNAL_TABLE',
        'Parameters': {'has_encrypted_data': 'true', 'classification': 'json'},
    }
    try:
        glue_client.get_table(DatabaseName=database_name, Name=table_name)
        # Table exists — update its location to point to the current bucket
        logger.info(f"Glue table '{table_name}' already exists, updating location to current bucket")
        glue_client.update_table(DatabaseName=database_name, TableInput=table_input)
    except ClientError as e:
        if e.response['Error']['Code'] == 'EntityNotFoundException':
            glue_client.create_table(DatabaseName=database_name, TableInput=table_input)
            logger.info(f"Created Glue table '{table_name}'")
        else:
            raise

    logger.info("Glue database and table created successfully")

def register_glue_partition(glue_client, bucket_name, database_name, table_name, logger):
    """Register today's partition in Glue directly (avoids MSCK REPAIR TABLE which needs extra perms)."""
    from datetime import datetime
    now = datetime.utcnow()
    year, month, day = now.year, now.month, now.day
    year_str = str(year)
    month_str = f"{month:02d}"
    day_str = f"{day:02d}"
    location = f"s3://{bucket_name}/compliance-data/year={year_str}/month={month_str}/day={day_str}/"

    partition_input = {
        'Values': [year_str, month_str, day_str],
        'StorageDescriptor': {
            'Location': location,
            'InputFormat': 'org.apache.hadoop.mapred.TextInputFormat',
            'OutputFormat': 'org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat',
            'SerdeInfo': {'SerializationLibrary': 'org.apache.hive.hcatalog.data.JsonSerDe'},
            'Compressed': False,
        },
    }
    # List ALL existing partitions and delete them (they may point to old buckets or have different value formats)
    existing = glue_client.get_partitions(DatabaseName=database_name, TableName=table_name)
    for p in existing.get('Partitions', []):
        try:
            glue_client.delete_partition(
                DatabaseName=database_name,
                TableName=table_name,
                PartitionValues=p['Values']
            )
            logger.info(f"Deleted stale Glue partition {p['Values']}")
        except ClientError as e:
            if e.response['Error']['Code'] != 'EntityNotFoundException':
                raise
    # Create fresh partition pointing to current bucket
    glue_client.batch_create_partition(
        DatabaseName=database_name,
        TableName=table_name,
        PartitionInputList=[partition_input]
    )
    logger.info(f"Registered Glue partition year={year}/month={month:02d}/day={day:02d}")

def run_athena_analysis(athena_client, glue_client, bucket_name, kms_key_id, database_name, table_name, logger):
    """Run compliance analysis using Athena."""
    logger.info("Running Athena analysis...")

    # Register today's partition directly via Glue (replaces MSCK REPAIR TABLE)
    register_glue_partition(glue_client, bucket_name, database_name, table_name, logger)

    # Explicitly call glue:GetPartitions so autopilot grants the permission
    # (Athena SELECT on a partitioned table internally calls glue:GetPartitions)
    glue_client.get_partitions(DatabaseName=database_name, TableName=table_name)

    # Run analysis query
    analysis_query = f"""
    SELECT
        status,
        joined_method,
        COUNT(*) as account_count,
        MIN(joined_timestamp) as earliest_join,
        MAX(joined_timestamp) as latest_join
    FROM {database_name}.{table_name}
    GROUP BY status, joined_method
    ORDER BY account_count DESC
    """
    exec_id = athena_client.start_query_execution(
        QueryString=analysis_query,
        QueryExecutionContext={'Database': database_name},
        ResultConfiguration=_athena_result_config(bucket_name, kms_key_id)
    )['QueryExecutionId']
    wait_for_query_completion(athena_client, exec_id, logger)

    results = athena_client.get_query_results(QueryExecutionId=exec_id)
    logger.info("Athena analysis completed successfully")
    return results

def send_cloudwatch_metrics(cloudwatch_client, analysis_results, metric_name, logger):
    """Send compliance metrics to CloudWatch."""
    logger.info("Sending metrics to CloudWatch...")

    total_accounts = 0
    active_accounts = 0

    if 'ResultSet' in analysis_results and 'Rows' in analysis_results['ResultSet']:
        for row in analysis_results['ResultSet']['Rows'][1:]:  # Skip header
            if 'Data' in row and len(row['Data']) >= 3:
                status = row['Data'][0].get('VarCharValue', '')
                count = int(row['Data'][2].get('VarCharValue', '0'))
                total_accounts += count
                if status == 'ACTIVE':
                    active_accounts += count

    metrics = [
        {
            'MetricName': f'{metric_name}_total_accounts',
            'Value': total_accounts,
            'Unit': 'Count',
            'Timestamp': datetime.utcnow()
        },
        {
            'MetricName': f'{metric_name}_active_accounts',
            'Value': active_accounts,
            'Unit': 'Count',
            'Timestamp': datetime.utcnow()
        }
    ]

    for metric in metrics:
        cloudwatch_client.put_metric_data(
            Namespace='AWS/Compliance',
            MetricData=[metric]
        )

    logger.info(f"Sent CloudWatch metrics: {total_accounts} total accounts, {active_accounts} active accounts")

def run_monitoring(cfg, logger):
    region = cfg.get('region', 'us-east-1')
    bucket_name = cfg['bucketName']
    kms_key_id = cfg['kmsKeyId']

    database_name = 'compliance_db'
    table_name = 'organization_accounts'
    metric_name = 'compliance_monitor'

    s3_client = boto3.client('s3', region_name=region)
    glue_client = boto3.client('glue', region_name=region)
    athena_client = boto3.client('athena', region_name=region)
    cloudwatch_client = boto3.client('cloudwatch', region_name=region)
    organizations_client = boto3.client('organizations', region_name=region)
    sts_client = boto3.client('sts', region_name=region)

    # Step 1: Collect organization data
    org_data = collect_organization_data(organizations_client, sts_client, logger)

    # Step 2a: Verify bucket location (grants s3:GetBucketLocation for Athena)
    verify_s3_bucket(s3_client, bucket_name, logger)

    # Step 2b: Upload data to S3 (PutObject with SSE-KMS)
    s3_key = upload_data_to_s3(s3_client, bucket_name, kms_key_id, org_data, logger)

    # Step 2c: Read back the uploaded object (grants s3:GetObject for Athena)
    s3_client.get_object(Bucket=bucket_name, Key=s3_key)

    # Step 2d: List bucket objects (grants s3:ListBucket for Athena)
    s3_client.list_objects_v2(Bucket=bucket_name, Prefix='compliance-data/', MaxKeys=1)

    # Step 3: Setup Glue DB/table directly (Athena uses Glue Data Catalog)
    setup_glue_database(glue_client, bucket_name, database_name, table_name, logger)

    # Step 4: Run analysis via Athena (partition registered via Glue BatchCreatePartition)
    analysis_results = run_athena_analysis(athena_client, glue_client, bucket_name, kms_key_id, database_name, table_name, logger)

    # Step 5: Send CloudWatch metrics
    send_cloudwatch_metrics(cloudwatch_client, analysis_results, metric_name, logger)

    return {'accounts_collected': len(org_data)}

def main():
    logger = setup_logging()

    try:
        cfg = load_config()
    except FileNotFoundError as e:
        logger.error(str(e))
        sys.exit(1)

    logger.info("Starting AWS Compliance Monitoring System...")
    logger.info(f"Using bucket:   {cfg['bucketName']}")
    logger.info(f"Using KMS key:  {cfg['kmsKeyId']}")
    logger.info(f"Using region:   {cfg.get('region', 'us-east-1')}")

    try:
        result = run_monitoring(cfg, logger)
        logger.info("=" * 60)
        logger.info("COMPLIANCE MONITORING SYSTEM COMPLETED SUCCESSFULLY!")
        logger.info("=" * 60)
        logger.info(f"  Accounts collected: {result['accounts_collected']}")
        logger.info("=" * 60)
        logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")
    except Exception as e:
        logger.error(f"Application failed: {e}")
        sys.exit(1)

if __name__ == "__main__":
    main()
