# Audit Log Archiver — Java

A Java service that reads audit log events from an SQS queue, enriches them
with user metadata from DynamoDB, signs them with KMS, and archives the signed
records to S3 Glacier. A secondary path publishes compliance summaries to SNS
and writes structured logs to CloudWatch Logs.

## Architecture

```
SQS (audit-events)
  └─► AuditLogProcessor (this app)
        ├─► DynamoDB (user-profiles) — enrich with user metadata
        ├─► KMS — sign each log record
        ├─► S3 Glacier (audit-archive) — long-term retention
        ├─► SNS (compliance-alerts) — anomaly notifications
        └─► CloudWatch Logs — structured audit trail
```

## Components

| File | Description |
|------|-------------|
| `AppConfig.java` | Configuration via environment variables and AWS SSM |
| `SqsEventReader.java` | SQS long-poll consumer with visibility extension |
| `UserProfileEnricher.java` | DynamoDB lookup for user metadata enrichment |
| `KmsRecordSigner.java` | KMS GenerateDataKey + sign audit records |
| `AuditLogProcessor.java` | Main orchestrator — wires all components together |

## CDK Stack

The CDK stack (`cdk/`) provisions:
- SQS queue (`audit-events`) with DLQ
- DynamoDB table (`user-profiles`, partition key: `user_id`)
- S3 bucket with Glacier lifecycle rule
- KMS key for audit record signing
- SNS topic for compliance alerts
- CloudWatch log group
- IAM role for the processor

## Building

```bash
mvn clean package -q
```

## Running

```bash
export AUDIT_QUEUE_URL=https://sqs.us-east-1.amazonaws.com/123456789012/audit-events
export USER_PROFILES_TABLE=user-profiles
export ARCHIVE_BUCKET=audit-archive-us-east-1
export KMS_KEY_ID=arn:aws:kms:us-east-1:123456789012:key/mrk-...
export COMPLIANCE_TOPIC_ARN=arn:aws:sns:us-east-1:123456789012:compliance-alerts
java -jar target/audit-log-archiver.jar
```

## Deploying

```bash
cd cdk
npm install
npx cdk deploy AuditLogArchiverStack
```
