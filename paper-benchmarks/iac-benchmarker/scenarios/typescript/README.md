# Feature Flag Service — TypeScript

A TypeScript service that manages feature flags stored in DynamoDB, evaluates
them for incoming requests, caches results in ElastiCache (Redis), and streams
flag evaluation events to Kinesis for analytics. Configuration is loaded from
AWS AppConfig; secrets from Secrets Manager.

## Architecture

```
HTTP clients → FeatureFlagService (this app)
                    ├─► DynamoDB (feature-flags) — flag definitions
                    ├─► ElastiCache Redis — evaluation cache
                    ├─► Kinesis (flag-events) — evaluation audit stream
                    ├─► AppConfig — dynamic configuration
                    └─► Secrets Manager — Redis auth token
```

## Components

| File | Description |
|------|-------------|
| `config.ts` | Configuration loader (env vars + Secrets Manager + AppConfig) |
| `dynamodb-client.ts` | DynamoDB CRUD for feature flag definitions |
| `kinesis-publisher.ts` | Kinesis PutRecords publisher for evaluation events |
| `flag-evaluator.ts` | Core flag evaluation logic with targeting rules |
| `server.ts` | Express HTTP server — wires all components, exposes REST API |

## CDK Stack

The CDK stack (`cdk/`) provisions:
- DynamoDB table (`feature-flags`, partition key: `flag_key`)
- Kinesis Data Stream (`flag-events`, 2 shards)
- ElastiCache Redis cluster (single-node)
- Secrets Manager secret for Redis auth token
- AppConfig application + environment + configuration profile
- IAM role for the service

## Running Locally

```bash
npm install
export FEATURE_FLAGS_TABLE=feature-flags
export KINESIS_STREAM_NAME=flag-events
export REDIS_SECRET_ARN=arn:aws:secretsmanager:us-east-1:123456789012:secret:redis-auth
export APPCONFIG_APP_ID=my-app
export APPCONFIG_ENV_ID=prod
export APPCONFIG_PROFILE_ID=feature-flag-service
npm start
```

## Deploying

```bash
cd cdk
npm install
npx cdk deploy FeatureFlagServiceStack
```
