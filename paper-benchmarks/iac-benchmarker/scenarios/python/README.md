# Document Processing Pipeline — Python

A serverless document ingestion and processing pipeline built with AWS services.
Documents are uploaded to S3, which triggers an SQS queue. A worker process
polls the queue, extracts metadata, stores it in DynamoDB, and publishes
processing results to an SNS topic.

## Architecture

```
S3 (uploads)
  └─► SQS (document-queue)
        └─► Worker (this app)
              ├─► DynamoDB (document-metadata)
              ├─► S3 (processed-output)
              └─► SNS (processing-results)
```

## Components

| File | Description |
|------|-------------|
| `config.py` | Configuration loader (env vars + SSM Parameter Store) |
| `s3_client.py` | S3 upload/download helpers |
| `sqs_worker.py` | SQS polling loop and message dispatch |
| `dynamodb_store.py` | DynamoDB read/write for document metadata |
| `processor.py` | Main entry point — orchestrates the pipeline |

## CDK Stack

The CDK stack (`cdk/`) provisions:
- S3 bucket for uploads and processed output
- SQS queue with dead-letter queue
- DynamoDB table (`document-metadata`, partition key: `document_id`)
- SNS topic for result notifications
- IAM role for the worker process

## Running Locally

```bash
pip install -r requirements.txt
export DOCUMENT_BUCKET=my-docs-bucket
export DOCUMENT_QUEUE_URL=https://sqs.us-east-1.amazonaws.com/123456789012/document-queue
export METADATA_TABLE=document-metadata
export RESULTS_TOPIC_ARN=arn:aws:sns:us-east-1:123456789012:processing-results
python processor.py
```

## Deploying

```bash
cd cdk
npm install
npx cdk deploy DocumentPipelineStack
```
