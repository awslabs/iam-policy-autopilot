#!/usr/bin/env node

/**
 * AWS Compliance Monitoring System — data-plane script (CDK-refactored)
 *
 * Infrastructure (KMS key, S3 bucket) is provisioned by the CDK stack in
 * ../cdk/lib/stack.ts.  Deploy it first:
 *
 *   cd ../cdk && bash deploy.sh
 *
 * That writes ../config.json with the stack outputs.  Then just run:
 *
 *   npx ts-node script.ts
 *
 * Services used (data-plane only):
 *   s3              : GetBucketLocation, PutObject (SSE-KMS)
 *   glue            : GetDatabase, CreateDatabase, GetTable, CreateTable
 *   athena          : StartQueryExecution, GetQueryExecution, GetQueryResults
 *   cloudwatch      : PutMetricData
 *   organizations   : ListAccounts (graceful fallback if not in org)
 *   sts             : GetCallerIdentity (fallback for org data)
 */

import * as fs from 'fs';
import * as path from 'path';
import {
  S3Client,
  PutObjectCommand,
  GetObjectCommand,
  ListObjectsV2Command,
  GetBucketLocationCommand,
  ServerSideEncryption,
} from '@aws-sdk/client-s3';
import {
  AthenaClient,
  StartQueryExecutionCommand,
  GetQueryExecutionCommand,
  GetQueryResultsCommand,
  QueryExecutionState,
} from '@aws-sdk/client-athena';
import {
  CloudWatchClient,
  PutMetricDataCommand,
  StandardUnit,
} from '@aws-sdk/client-cloudwatch';
import {
  GlueClient,
  GetDatabaseCommand,
  CreateDatabaseCommand,
  GetTableCommand,
  CreateTableCommand,
  UpdateTableCommand,
  BatchCreatePartitionCommand,
  DeletePartitionCommand,
  GetPartitionsCommand,
} from '@aws-sdk/client-glue';
import {
  OrganizationsClient,
  ListAccountsCommand,
} from '@aws-sdk/client-organizations';
import {
  STSClient,
  GetCallerIdentityCommand,
} from '@aws-sdk/client-sts';

// ── Config loading ─────────────────────────────────────────────────────────────

interface RunConfig {
  bucketName: string;
  kmsKeyId: string;
  kmsKeyArn: string;
  region: string;
}

function loadConfig(): RunConfig {
  const configPath = path.resolve(__dirname, '..', 'config.json');
  if (!fs.existsSync(configPath)) {
    throw new Error(
      `config.json not found at ${configPath}.\n` +
      'Deploy the CDK stack first:\n' +
      '  cd ../cdk && bash deploy.sh',
    );
  }
  return JSON.parse(fs.readFileSync(configPath, 'utf-8')) as RunConfig;
}

// ── Logger ────────────────────────────────────────────────────────────────────

class Logger {
  info(message: string): void {
    console.log(`${new Date().toISOString()} - INFO - ${message}`);
  }
  error(message: string): void {
    console.error(`${new Date().toISOString()} - ERROR - ${message}`);
  }
  warn(message: string): void {
    console.warn(`${new Date().toISOString()} - WARN - ${message}`);
  }
}

const logger = new Logger();

// ── Account info ──────────────────────────────────────────────────────────────

interface AccountInfo {
  account_id: string;
  account_name: string;
  email: string;
  status: string;
  joined_method: string;
  joined_timestamp: string;
  collection_time: string;
}

// ── Collect organization data ─────────────────────────────────────────────────

async function collectOrganizationData(
  orgClient: OrganizationsClient,
  stsClient: STSClient,
): Promise<AccountInfo[]> {
  logger.info('Collecting organization data...');
  const accountsData: AccountInfo[] = [];

  try {
    let nextToken: string | undefined;
    do {
      const response = await orgClient.send(new ListAccountsCommand({ NextToken: nextToken }));
      for (const account of response.Accounts ?? []) {
        accountsData.push({
          account_id: account.Id ?? '',
          account_name: account.Name ?? '',
          email: account.Email ?? '',
          status: account.Status ?? '',
          joined_method: account.JoinedMethod ?? '',
          joined_timestamp: account.JoinedTimestamp?.toISOString() ?? new Date().toISOString(),
          collection_time: new Date().toISOString(),
        });
      }
      nextToken = response.NextToken;
    } while (nextToken);
  } catch (err: unknown) {
    const e = err as { name?: string };
    if (e.name === 'AWSOrganizationsNotInUseException' ||
        e.name === 'AccessDeniedException') {
      logger.warn('Organizations not available, using current account only');
      const identity = await stsClient.send(new GetCallerIdentityCommand({}));
      accountsData.push({
        account_id: identity.Account ?? '',
        account_name: 'Current Account',
        email: 'unknown@example.com',
        status: 'ACTIVE',
        joined_method: 'CREATED',
        joined_timestamp: new Date().toISOString(),
        collection_time: new Date().toISOString(),
      });
    } else {
      throw err;
    }
  }

  logger.info(`Collected data for ${accountsData.length} accounts`);
  return accountsData;
}

// ── Verify S3 bucket (grants s3:GetBucketLocation for Athena) ────────────────

async function verifyS3Bucket(s3Client: S3Client, bucketName: string): Promise<void> {
  const response = await s3Client.send(new GetBucketLocationCommand({ Bucket: bucketName }));
  const location = response.LocationConstraint ?? 'us-east-1';
  logger.info(`Bucket location: ${location}`);
}

// ── Upload data to S3 ─────────────────────────────────────────────────────────

async function uploadDataToS3(
  s3Client: S3Client,
  bucketName: string,
  kmsKeyId: string,
  data: AccountInfo[],
): Promise<string> {
  logger.info('Uploading data to S3...');

  const jsonLines = data.map(r => JSON.stringify(r)).join('\n');
  const content = Buffer.from(jsonLines, 'utf-8');

  const now = new Date();
  const key = `compliance-data/year=${now.getFullYear()}/month=${String(now.getMonth() + 1).padStart(2, '0')}/day=${String(now.getDate()).padStart(2, '0')}/accounts_${Math.floor(Date.now() / 1000)}.json`;

  await s3Client.send(new PutObjectCommand({
    Bucket: bucketName,
    Key: key,
    Body: content,
    ContentType: 'application/json',
    ServerSideEncryption: ServerSideEncryption.aws_kms,
    SSEKMSKeyId: kmsKeyId,
  }));

  logger.info(`Uploaded data to S3: s3://${bucketName}/${key}`);
  return key;
}

// ── Athena helpers ────────────────────────────────────────────────────────────

async function executeAthenaQuery(
  athenaClient: AthenaClient,
  query: string,
  database: string | undefined,
  bucketName: string,
  kmsKeyId: string,
): Promise<string> {
  const response = await athenaClient.send(new StartQueryExecutionCommand({
    QueryString: query,
    ResultConfiguration: {
      OutputLocation: `s3://${bucketName}/query-results/`,
      EncryptionConfiguration: {
        EncryptionOption: 'SSE_KMS',
        KmsKey: kmsKeyId,
      },
    },
    QueryExecutionContext: database ? { Database: database } : undefined,
  }));
  return response.QueryExecutionId!;
}

async function waitForQueryCompletion(
  athenaClient: AthenaClient,
  queryExecutionId: string,
): Promise<void> {
  const maxWaitMs = 5 * 60 * 1000;
  const startTime = Date.now();

  while (Date.now() - startTime < maxWaitMs) {
    const response = await athenaClient.send(new GetQueryExecutionCommand({
      QueryExecutionId: queryExecutionId,
    }));

    const state = response.QueryExecution?.Status?.State;

    if (state === QueryExecutionState.SUCCEEDED) {
      return;
    } else if (state === QueryExecutionState.FAILED || state === QueryExecutionState.CANCELLED) {
      const reason = response.QueryExecution?.Status?.StateChangeReason ?? 'Unknown';
      throw new Error(`Query ${state.toLowerCase()}: ${reason}`);
    }

    await new Promise(resolve => setTimeout(resolve, 5000));
  }

  throw new Error('Query timed out after 5 minutes');
}

async function setupGlueDatabase(
  glueClient: GlueClient,
  bucketName: string,
  databaseName: string,
  tableName: string,
): Promise<void> {
  logger.info('Setting up Glue database and table (Athena uses Glue Data Catalog)...');

  // Create database if it doesn't exist
  try {
    await glueClient.send(new GetDatabaseCommand({ Name: databaseName }));
    logger.info(`Glue database '${databaseName}' already exists`);
  } catch (err: unknown) {
    const e = err as { name?: string };
    if (e.name === 'EntityNotFoundException') {
      await glueClient.send(new CreateDatabaseCommand({
        DatabaseInput: {
          Name: databaseName,
          Description: 'Compliance monitoring database',
        },
      }));
      logger.info(`Created Glue database '${databaseName}'`);
    } else {
      throw err;
    }
  }

  // Build table input (used for both create and update)
  const tableInput = {
    Name: tableName,
    TableType: 'EXTERNAL_TABLE',
    StorageDescriptor: {
      Columns: [
        { Name: 'account_id', Type: 'string' },
        { Name: 'account_name', Type: 'string' },
        { Name: 'email', Type: 'string' },
        { Name: 'status', Type: 'string' },
        { Name: 'joined_method', Type: 'string' },
        { Name: 'joined_timestamp', Type: 'string' },
        { Name: 'collection_time', Type: 'string' },
      ],
      Location: `s3://${bucketName}/compliance-data/`,
      InputFormat: 'org.apache.hadoop.mapred.TextInputFormat',
      OutputFormat: 'org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat',
      SerdeInfo: {
        SerializationLibrary: 'org.apache.hive.hcatalog.data.JsonSerDe',
      },
      Compressed: false,
    },
    PartitionKeys: [
      { Name: 'year', Type: 'string' },
      { Name: 'month', Type: 'string' },
      { Name: 'day', Type: 'string' },
    ],
    Parameters: {
      has_encrypted_data: 'true',
      classification: 'json',
    },
  };

  // Create or update table (glue:GetTable + glue:CreateTable + glue:UpdateTable)
  try {
    await glueClient.send(new GetTableCommand({ DatabaseName: databaseName, Name: tableName }));
    // Table exists — update its location to point to the current bucket
    logger.info(`Glue table '${databaseName}.${tableName}' already exists, updating location`);
    await glueClient.send(new UpdateTableCommand({ DatabaseName: databaseName, TableInput: tableInput }));
  } catch (err: unknown) {
    const e = err as { name?: string };
    if (e.name === 'EntityNotFoundException') {
      await glueClient.send(new CreateTableCommand({ DatabaseName: databaseName, TableInput: tableInput }));
      logger.info(`Created Glue table '${databaseName}.${tableName}'`);
    } else {
      throw err;
    }
  }

  logger.info('Glue database and table ready');
}

async function registerGluePartition(
  glueClient: GlueClient,
  bucketName: string,
  databaseName: string,
  tableName: string,
): Promise<void> {
  const now = new Date();
  const year = now.getUTCFullYear();
  const month = now.getUTCMonth() + 1;
  const day = now.getUTCDate();
  const location = `s3://${bucketName}/compliance-data/year=${year}/month=${String(month).padStart(2, '0')}/day=${String(day).padStart(2, '0')}/`;

  const partitionInput = {
    Values: [String(year), String(month).padStart(2, '0'), String(day).padStart(2, '0')],
    StorageDescriptor: {
      Location: location,
      InputFormat: 'org.apache.hadoop.mapred.TextInputFormat',
      OutputFormat: 'org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat',
      SerdeInfo: {
        SerializationLibrary: 'org.apache.hive.hcatalog.data.JsonSerDe',
      },
      Compressed: false,
    },
  };

  // List ALL existing partitions and delete them (may point to old buckets or have different value formats)
  const existingResp = await glueClient.send(new GetPartitionsCommand({
    DatabaseName: databaseName,
    TableName: tableName,
  }));
  for (const p of existingResp.Partitions ?? []) {
    try {
      await glueClient.send(new DeletePartitionCommand({
        DatabaseName: databaseName,
        TableName: tableName,
        PartitionValues: p.Values,
      }));
      logger.info(`Deleted stale Glue partition ${JSON.stringify(p.Values)}`);
    } catch (err: unknown) {
      const e = err as { name?: string };
      if (e.name !== 'EntityNotFoundException') {
        throw err;
      }
    }
  }
  // Create fresh partition pointing to current bucket
  const batchResp = await glueClient.send(new BatchCreatePartitionCommand({
    DatabaseName: databaseName,
    TableName: tableName,
    PartitionInputList: [partitionInput],
  }));
  const batchErrors = batchResp.Errors ?? [];
  if (batchErrors.length > 0) {
    throw new Error(`Failed to create partition: ${JSON.stringify(batchErrors[0])}`);
  }
  logger.info(`Registered Glue partition year=${year}/month=${String(month).padStart(2, '0')}/day=${String(day).padStart(2, '0')}`);
}

async function runAthenaAnalysis(
  glueClient: GlueClient,
  athenaClient: AthenaClient,
  bucketName: string,
  kmsKeyId: string,
  databaseName: string,
  tableName: string,
): Promise<Record<string, unknown>> {
  logger.info('Running Athena analysis...');

  // Register today's partition directly via Glue (replaces MSCK REPAIR TABLE)
  await registerGluePartition(glueClient, bucketName, databaseName, tableName);

  // Explicitly call glue:GetPartitions so autopilot grants the permission
  // (Athena SELECT on a partitioned table internally calls glue:GetPartitions)
  await glueClient.send(new GetPartitionsCommand({
    DatabaseName: databaseName,
    TableName: tableName,
  }));

  // Run analysis query
  const analysisQuery = `
    SELECT
      status,
      joined_method,
      COUNT(*) as account_count,
      MIN(joined_timestamp) as earliest_join,
      MAX(joined_timestamp) as latest_join
    FROM ${databaseName}.${tableName}
    GROUP BY status, joined_method
    ORDER BY account_count DESC
  `;
  const execId = await executeAthenaQuery(athenaClient, analysisQuery, databaseName, bucketName, kmsKeyId);
  await waitForQueryCompletion(athenaClient, execId);

  const results = await athenaClient.send(new GetQueryResultsCommand({
    QueryExecutionId: execId,
  }));

  logger.info('Athena analysis completed successfully');
  return results as unknown as Record<string, unknown>;
}

// ── CloudWatch metrics ────────────────────────────────────────────────────────

async function sendCloudWatchMetrics(
  cwClient: CloudWatchClient,
  analysisResults: Record<string, unknown>,
  metricName: string,
): Promise<void> {
  logger.info('Sending metrics to CloudWatch...');

  let totalAccounts = 0;
  let activeAccounts = 0;

  const resultSet = (analysisResults as { ResultSet?: { Rows?: Array<{ Data?: Array<{ VarCharValue?: string }> }> } }).ResultSet;
  if (resultSet?.Rows && resultSet.Rows.length > 1) {
    for (const row of resultSet.Rows.slice(1)) {
      const data = row.Data ?? [];
      if (data.length >= 3) {
        const status = data[0]?.VarCharValue ?? '';
        const count = parseInt(data[2]?.VarCharValue ?? '0', 10) || 0;
        totalAccounts += count;
        if (status === 'ACTIVE') {
          activeAccounts += count;
        }
      }
    }
  }

  const now = new Date();
  const metrics = [
    { MetricName: `${metricName}_total_accounts`, Value: totalAccounts },
    { MetricName: `${metricName}_active_accounts`, Value: activeAccounts },
  ];

  for (const metric of metrics) {
    await cwClient.send(new PutMetricDataCommand({
      Namespace: 'AWS/Compliance',
      MetricData: [{
        MetricName: metric.MetricName,
        Value: metric.Value,
        Unit: StandardUnit.Count,
        Timestamp: now,
      }],
    }));
  }

  logger.info(`Sent CloudWatch metrics: ${totalAccounts} total accounts, ${activeAccounts} active accounts`);
}

// ── Main logic ────────────────────────────────────────────────────────────────

async function runMonitoring(cfg: RunConfig): Promise<void> {
  const { bucketName, kmsKeyId, region } = cfg;

  const s3Client = new S3Client({ region });
  const glueClient = new GlueClient({ region });
  const athenaClient = new AthenaClient({ region });
  const cwClient = new CloudWatchClient({ region });
  const orgClient = new OrganizationsClient({ region });
  const stsClient = new STSClient({ region });

  const databaseName = 'compliance_db';
  const tableName = 'organization_accounts';
  const metricName = 'compliance_monitor';

  // Step 1: Collect organization data
  const orgData = await collectOrganizationData(orgClient, stsClient);

  // Step 2a: Verify bucket location (grants s3:GetBucketLocation for Athena)
  await verifyS3Bucket(s3Client, bucketName);

  // Step 2b: Upload data to S3 (PutObject with SSE-KMS)
  const s3Key = await uploadDataToS3(s3Client, bucketName, kmsKeyId, orgData);

  // Step 2c: Read back the uploaded object (grants s3:GetObject for Athena)
  await s3Client.send(new GetObjectCommand({ Bucket: bucketName, Key: s3Key }));

  // Step 2d: List bucket objects (grants s3:ListBucket for Athena)
  await s3Client.send(new ListObjectsV2Command({ Bucket: bucketName, Prefix: 'compliance-data/', MaxKeys: 1 }));

  // Step 3: Setup Glue DB/table directly (Athena uses Glue Data Catalog)
  await setupGlueDatabase(glueClient, bucketName, databaseName, tableName);

  // Step 4: Run analysis (partition registered via Glue BatchCreatePartition)
  const analysisResults = await runAthenaAnalysis(glueClient, athenaClient, bucketName, kmsKeyId, databaseName, tableName);

  // Step 5: Send CloudWatch metrics
  await sendCloudWatchMetrics(cwClient, analysisResults, metricName);
}

// ── Entry point ───────────────────────────────────────────────────────────────

async function main(): Promise<void> {
  let cfg: RunConfig;
  try {
    cfg = loadConfig();
  } catch (error) {
    logger.error(`Configuration error: ${error}`);
    process.exit(1);
    return;
  }

  logger.info('Starting AWS Compliance Monitoring System...');
  logger.info(`Using bucket:  ${cfg.bucketName}`);
  logger.info(`Using KMS key: ${cfg.kmsKeyId}`);
  logger.info(`Using region:  ${cfg.region}`);

  try {
    await runMonitoring(cfg);

    logger.info('='.repeat(60));
    logger.info('COMPLIANCE MONITORING SYSTEM COMPLETED SUCCESSFULLY!');
    logger.info('='.repeat(60));
    logger.info('To destroy infrastructure, run: cd ../cdk && npx cdk destroy');
  } catch (error) {
    logger.error(`Application failed: ${error}`);
    process.exit(1);
  }
}

if (require.main === module) {
  main().catch((error) => {
    console.error('Fatal error:', error);
    process.exit(1);
  });
}
