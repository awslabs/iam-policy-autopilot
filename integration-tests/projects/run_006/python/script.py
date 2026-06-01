#!/usr/bin/env python3
import boto3, hashlib, json, logging, os, sys, time
from datetime import datetime
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

def upload_document(s3_client, dynamodb, bucket_name, table_name, kms_key_id, file_path, document_name, logger):
    """Upload document to S3 and store metadata in DynamoDB."""
    with open(file_path, 'rb') as f:
        file_content = f.read()
    file_hash = hashlib.sha256(file_content).hexdigest()
    document_id = hashlib.sha256(f"{document_name}_{time.time()}".encode()).hexdigest()[:16]
    s3_key = f"documents/{document_id}/{document_name}"

    s3_client.put_object(
        Bucket=bucket_name, Key=s3_key, Body=file_content,
        ServerSideEncryption='aws:kms', SSEKMSKeyId=kms_key_id,
        Metadata={'document-id': document_id, 'original-name': document_name}
    )
    logger.info(f"Uploaded to s3://{bucket_name}/{s3_key}")

    table = dynamodb.Table(table_name)
    table.put_item(Item={
        'document_id': document_id, 'document_name': document_name,
        's3_bucket': bucket_name, 's3_key': s3_key,
        'file_hash': file_hash, 'file_size': len(file_content),
        'upload_timestamp': datetime.now().isoformat(), 'status': 'active'
    })
    logger.info(f"Stored metadata in DynamoDB for document_id={document_id}")
    return document_id, s3_key, file_hash

def log_operation(logs_client, log_group_name, operation, document_id, document_name, status, logger):
    """Log operation to CloudWatch Logs."""
    log_stream_name = f"document-operations-{datetime.now().strftime('%Y-%m-%d')}"
    try:
        logs_client.create_log_stream(logGroupName=log_group_name, logStreamName=log_stream_name)
    except ClientError as e:
        if e.response['Error']['Code'] != 'ResourceAlreadyExistsException':
            raise
    log_entry = json.dumps({'timestamp': datetime.now().isoformat(), 'operation': operation,
                            'document_id': document_id, 'document_name': document_name, 'status': status})
    logs_client.put_log_events(
        logGroupName=log_group_name, logStreamName=log_stream_name,
        logEvents=[{'timestamp': int(time.time() * 1000), 'message': log_entry}]
    )
    logger.info(f"Logged {operation} operation to CloudWatch")

def list_documents(dynamodb, table_name, logger):
    table = dynamodb.Table(table_name)
    response = table.scan()
    docs = response['Items']
    logger.info(f"Found {len(docs)} document(s) in DynamoDB")
    return docs

def download_document(s3_client, dynamodb, bucket_name, table_name, document_id, download_path, logger):
    table = dynamodb.Table(table_name)
    response = table.get_item(Key={'document_id': document_id})
    item = response['Item']
    s3_key = item['s3_key']
    stored_hash = item['file_hash']

    response = s3_client.get_object(Bucket=bucket_name, Key=s3_key)
    file_content = response['Body'].read()
    file_hash = hashlib.sha256(file_content).hexdigest()
    if file_hash != stored_hash:
        raise ValueError("File integrity check failed")

    with open(download_path, 'wb') as f:
        f.write(file_content)
    logger.info(f"Downloaded document to {download_path}")
    return item['document_name']

def run_demo(cfg, logger):
    region = cfg.get('region', 'us-east-1')
    sts_client  = boto3.client('sts',      region_name=region)
    s3_client   = boto3.client('s3',       region_name=region)
    dynamodb    = boto3.resource('dynamodb', region_name=region)
    logs_client = boto3.client('logs',     region_name=region)

    account_id = get_aws_account_id(sts_client)
    logger.info(f"AWS Account ID: {account_id}")

    # Create sample document
    sample_path = '/tmp/sample_document.txt'
    with open(sample_path, 'w') as f:
        f.write('This is a sample document for testing the secure document management system.')

    # Upload
    document_id, s3_key, file_hash = upload_document(
        s3_client, dynamodb, cfg['bucketName'], cfg['tableName'],
        cfg['kmsKeyId'], sample_path, 'sample_document.txt', logger
    )
    log_operation(logs_client, cfg['logGroupName'], 'UPLOAD', document_id, 'sample_document.txt', 'SUCCESS', logger)

    # List
    docs = list_documents(dynamodb, cfg['tableName'], logger)

    # Download
    download_path = '/tmp/downloaded_sample.txt'
    doc_name = download_document(
        s3_client, dynamodb, cfg['bucketName'], cfg['tableName'],
        document_id, download_path, logger
    )
    log_operation(logs_client, cfg['logGroupName'], 'DOWNLOAD', document_id, doc_name, 'SUCCESS', logger)

    return {'account_id': account_id, 'document_id': document_id, 'documents_count': len(docs)}

def main():
    logger = setup_logging()
    try:
        cfg = load_config()
    except FileNotFoundError as e:
        logger.error(str(e))
        sys.exit(1)

    logger.info("Starting Secure Document Management System...")
    logger.info(f"Using bucket:     {cfg['bucketName']}")
    logger.info(f"Using table:      {cfg['tableName']}")
    logger.info(f"Using KMS key:    {cfg['kmsKeyId']}")
    logger.info(f"Using log group:  {cfg['logGroupName']}")
    logger.info(f"Using region:     {cfg.get('region', 'us-east-1')}")

    try:
        result = run_demo(cfg, logger)
        logger.info("=" * 60)
        logger.info("APPLICATION COMPLETED SUCCESSFULLY!")
        logger.info("=" * 60)
        logger.info("Resources used:")
        logger.info(f"  - S3 Bucket:   {cfg['bucketName']}")
        logger.info(f"  - DynamoDB:    {cfg['tableName']}")
        logger.info(f"  - Log Group:   {cfg['logGroupName']}")
        logger.info("Summary:")
        logger.info(f"  - Document ID:    {result['document_id']}")
        logger.info(f"  - Total docs:     {result['documents_count']}")
        logger.info("=" * 60)
        logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")
    except Exception as e:
        logger.error(f"Application failed: {e}")
        sys.exit(1)

if __name__ == "__main__":
    main()
