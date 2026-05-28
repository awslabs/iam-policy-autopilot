#!/usr/bin/env node

import * as fs from 'fs';
import * as path from 'path';
import {
  STSClient,
  GetCallerIdentityCommand,
} from '@aws-sdk/client-sts';
import {
  XRayClient,
  PutEncryptionConfigCommand,
  EncryptionType,
} from '@aws-sdk/client-xray';

// ── Config loading ─────────────────────────────────────────────────────────────

interface RunConfig {
  clusterName: string;
  clusterArn: string;
  logGroupName: string;
  kmsKeyId: string;
  kmsKeyArn: string;
  resourceGroupName: string;
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

async function configureXRayEncryption(xrayClient: XRayClient): Promise<void> {
  await xrayClient.send(new PutEncryptionConfigCommand({
    Type: EncryptionType.NONE,
  }));
  logger.info('X-Ray encryption configured (Type=NONE)');
}

// ── Main logic ────────────────────────────────────────────────────────────────

async function runMLMonitoring(cfg: RunConfig): Promise<string> {
  const { region } = cfg;

  const stsClient  = new STSClient({ region });
  const xrayClient = new XRayClient({ region });

  // 1. Get account ID
  logger.info('Getting AWS account information...');
  const accountId = await getAwsAccountId(stsClient);
  logger.info(`AWS Account ID: ${accountId}`);

  // 2. Configure X-Ray encryption (data-plane runtime config)
  logger.info('Configuring X-Ray encryption...');
  await configureXRayEncryption(xrayClient);

  return accountId;
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

  logger.info('Starting ML Monitoring Platform...');
  logger.info(`Using ECS cluster:    ${cfg.clusterName}`);
  logger.info(`Using log group:      ${cfg.logGroupName}`);
  logger.info(`Using KMS key:        ${cfg.kmsKeyId}`);
  logger.info(`Using resource group: ${cfg.resourceGroupName}`);
  logger.info(`Using region:         ${cfg.region}`);

  try {
    const accountId = await runMLMonitoring(cfg);

    logger.info('='.repeat(60));
    logger.info('APPLICATION COMPLETED SUCCESSFULLY!');
    logger.info('='.repeat(60));
    logger.info('Resources used:');
    logger.info(`  - ECS Cluster:    ${cfg.clusterName}`);
    logger.info(`  - Log Group:      ${cfg.logGroupName}`);
    logger.info(`  - KMS Key:        ${cfg.kmsKeyId}`);
    logger.info(`  - Resource Group: ${cfg.resourceGroupName}`);
    logger.info('Summary:');
    logger.info(`  - AWS Account ID: ${accountId}`);
    logger.info(`  - Region:         ${cfg.region}`);
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
