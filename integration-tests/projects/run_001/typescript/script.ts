#!/usr/bin/env node

import * as fs from 'fs';
import * as path from 'path';
import {
  STSClient,
  GetCallerIdentityCommand,
} from '@aws-sdk/client-sts';
import {
  RedshiftDataClient,
  ExecuteStatementCommand,
  DescribeStatementCommand,
  StatusString,
} from '@aws-sdk/client-redshift-data';

// ── Config loading ─────────────────────────────────────────────────────────────

interface RunConfig {
  bucketName: string;
  redshiftClusterIdentifier: string;
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

// ── Data-plane helpers ────────────────────────────────────────────────────────

async function getAwsAccountId(stsClient: STSClient): Promise<string> {
  const response = await stsClient.send(new GetCallerIdentityCommand({}));
  if (!response.Account) {
    throw new Error('No account ID returned from STS');
  }
  return response.Account;
}

async function executeRedshiftStatement(
  rdClient: RedshiftDataClient,
  clusterIdentifier: string,
  database: string,
  dbUser: string,
  sql: string,
): Promise<string> {
  const response = await rdClient.send(new ExecuteStatementCommand({
    ClusterIdentifier: clusterIdentifier,
    Database: database,
    DbUser: dbUser,
    Sql: sql,
  }));
  const stmtId = response.Id!;
  logger.info(`Redshift Data API ExecuteStatement submitted, id=${stmtId}`);
  return stmtId;
}

async function waitForRedshiftStatement(
  rdClient: RedshiftDataClient,
  stmtId: string,
  pollIntervalMs: number = 2000,
  maxWaitMs: number = 60000,
): Promise<string> {
  const deadline = Date.now() + maxWaitMs;
  while (Date.now() < deadline) {
    const desc = await rdClient.send(new DescribeStatementCommand({ Id: stmtId }));
    const status = desc.Status as string;
    logger.info(`  Statement ${stmtId} status: ${status}`);
    if (status === StatusString.FINISHED) return status;
    if (status === StatusString.FAILED || status === StatusString.ABORTED) {
      logger.warn(`  Statement ended with status ${status}: ${desc.Error ?? 'unknown error'}`);
      return status;
    }
    await new Promise(resolve => setTimeout(resolve, pollIntervalMs));
  }
  logger.warn(`  Statement ${stmtId} did not finish within ${maxWaitMs}ms`);
  return 'TIMEOUT';
}

// ── Main logic ────────────────────────────────────────────────────────────────

async function runSecurityAnalytics(cfg: RunConfig): Promise<void> {
  const { redshiftClusterIdentifier: clusterIdentifier, region } = cfg;

  const stsClient = new STSClient({ region });
  const rdClient  = new RedshiftDataClient({ region });

  // ── STS: GetCallerIdentity ─────────────────────────────────────────────────
  logger.info('Getting AWS account information...');
  const accountId = await getAwsAccountId(stsClient);
  logger.info(`Using AWS Account ID: ${accountId}`);

  const database = 'securitydb';
  const dbUser   = 'adminuser';

  // ── Redshift Data API: 1. CREATE TABLE ────────────────────────────────────
  logger.info('Executing Redshift statement 1/3: CREATE TABLE security_events...');
  const createSQL = `
CREATE TABLE IF NOT EXISTS security_events (
    event_id    VARCHAR(64),
    event_type  VARCHAR(64),
    source_ip   VARCHAR(45),
    user_name   VARCHAR(128),
    timestamp   TIMESTAMP,
    severity    VARCHAR(16),
    description VARCHAR(512)
)`.trim();

  let stmtId = await executeRedshiftStatement(rdClient, clusterIdentifier, database, dbUser, createSQL);
  await waitForRedshiftStatement(rdClient, stmtId);

  // ── Redshift Data API: 2. INSERT data ─────────────────────────────────────
  logger.info('Executing Redshift statement 2/3: INSERT security events...');
  const insertSQL = `
INSERT INTO security_events
    (event_id, event_type, source_ip, user_name, timestamp, severity, description)
VALUES
    ('evt-001', 'LOGIN_FAILURE',        '192.168.1.100', 'user1', GETDATE(), 'HIGH',     'Multiple failed login attempts'),
    ('evt-002', 'DATA_ACCESS',          '10.0.0.50',     'user2', GETDATE(), 'MEDIUM',   'Unusual data access pattern'),
    ('evt-003', 'PRIVILEGE_ESCALATION', '172.16.0.1',    'user3', GETDATE(), 'CRITICAL', 'Unauthorized privilege escalation attempt')`.trim();

  stmtId = await executeRedshiftStatement(rdClient, clusterIdentifier, database, dbUser, insertSQL);
  await waitForRedshiftStatement(rdClient, stmtId);

  // ── Redshift Data API: 3. Analytics SELECT ────────────────────────────────
  logger.info('Executing Redshift statement 3/3: Analytics query on security_events...');
  const analyticsSQL = `
SELECT
    severity,
    COUNT(*)       AS event_count,
    MIN(timestamp) AS first_seen,
    MAX(timestamp) AS last_seen
FROM security_events
GROUP BY severity
ORDER BY event_count DESC`.trim();

  stmtId = await executeRedshiftStatement(rdClient, clusterIdentifier, database, dbUser, analyticsSQL);
  await waitForRedshiftStatement(rdClient, stmtId);

  logger.info('='.repeat(60));
  logger.info('APPLICATION COMPLETED SUCCESSFULLY!');
  logger.info('='.repeat(60));
  logger.info('Resources used (data-plane):');
  logger.info(`  - STS:           GetCallerIdentity (account: ${accountId})`);
  logger.info(`  - Redshift Data: ExecuteStatement x3 (cluster: ${clusterIdentifier})`);
  logger.info('='.repeat(60));
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

  logger.info('Starting AWS Security and Analytics Platform (data-plane)...');
  logger.info(`Using Redshift cluster: ${cfg.redshiftClusterIdentifier}`);
  logger.info(`Using region:           ${cfg.region}`);

  try {
    await runSecurityAnalytics(cfg);
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
