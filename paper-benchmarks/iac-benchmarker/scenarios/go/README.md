# Metrics Aggregation Service — Go

A high-throughput metrics collection and aggregation service. Agents push
time-series metrics to Kinesis Data Streams; this service consumes the stream,
aggregates values into one-minute windows, stores results in DynamoDB, and
archives raw data to S3 via Kinesis Firehose.

## Architecture

```
Agents → Kinesis Data Stream (metrics-stream)
              └─► Consumer (this app)
                    ├─► DynamoDB (metrics-aggregates)
                    ├─► S3 (metrics-archive via Firehose)
                    └─► CloudWatch (custom metrics)
```

## Components

| File | Description |
|------|-------------|
| `config.go` | Configuration via environment variables and AWS AppConfig |
| `kinesis_consumer.go` | Kinesis shard iterator and record polling |
| `aggregator.go` | Sliding-window aggregation logic (min/max/avg/p99) |
| `dynamodb_writer.go` | Batch-write aggregated metrics to DynamoDB |
| `main.go` | Entry point — wires consumer, aggregator, and writer |

## CDK Stack

The CDK stack (`cdk/`) provisions:
- Kinesis Data Stream (`metrics-stream`, 4 shards)
- DynamoDB table (`metrics-aggregates`, partition key: `metric_name`, sort key: `window_start`)
- S3 bucket for raw metric archives
- Kinesis Firehose delivery stream → S3
- CloudWatch log group and metric namespace
- IAM role for the consumer process

## Running Locally

```bash
export KINESIS_STREAM_NAME=metrics-stream
export METRICS_TABLE=metrics-aggregates
export ARCHIVE_BUCKET=metrics-archive-us-east-1
export AWS_REGION=us-east-1
go run .
```

## Deploying

```bash
cd cdk
npm install
npx cdk deploy MetricsAggregationStack
```
