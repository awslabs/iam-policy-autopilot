#!/usr/bin/env node

import * as fs from 'fs';
import * as path from 'path';
import {
  SecretsManagerClient,
  GetSecretValueCommand,
} from '@aws-sdk/client-secrets-manager';
import {
  SNSClient,
  PublishCommand,
} from '@aws-sdk/client-sns';
import {
  STSClient,
  GetCallerIdentityCommand,
} from '@aws-sdk/client-sts';

// ── Config loading ─────────────────────────────────────────────────────────────

interface RunConfig {
  topicArn: string;
  secretName: string;
  secretArn: string;
  kmsKeyId: string;
  kmsKeyArn: string;
  repoName: string;
  cloneUrl: string;
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

// ── Types ─────────────────────────────────────────────────────────────────────

interface MonitoringResult {
  accountId: string;
  topicArn: string;
  secretName: string;
  repoName: string;
  cloneUrl: string;
  region: string;
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

async function retrieveSecret(
  secretsClient: SecretsManagerClient,
  secretName: string,
): Promise<Record<string, unknown>> {
  logger.info(`Retrieving configuration from Secrets Manager: ${secretName}`);
  const response = await secretsClient.send(new GetSecretValueCommand({
    SecretId: secretName,
  }));

  if (!response.SecretString) {
    throw new Error('No secret string returned from Secrets Manager');
  }

  const secretData = JSON.parse(response.SecretString) as Record<string, unknown>;
  logger.info('Successfully retrieved and decrypted configuration from Secrets Manager');
  return secretData;
}

async function sendNotification(
  snsClient: SNSClient,
  topicArn: string,
  repoName: string,
  cloneUrl: string,
): Promise<void> {
  logger.info(`Sending repository notification via SNS to topic: ${topicArn}`);

  const message = {
    default: `Secure repository '${repoName}' is configured and ready.`,
    email: `Repository Monitoring Alert\n\nRepository: ${repoName}\nClone URL: ${cloneUrl}\n\nSecurity features: KMS encryption, SNS notifications, Secrets Manager integration.`,
  };

  await snsClient.send(new PublishCommand({
    TopicArn: topicArn,
    Message: JSON.stringify(message),
    MessageStructure: 'json',
    Subject: `Repository Ready: ${repoName}`,
  }));

  logger.info('Notification sent successfully');
}

// ── Main logic ────────────────────────────────────────────────────────────────

async function runSecureRepoMonitoring(cfg: RunConfig): Promise<MonitoringResult> {
  const { topicArn, secretName, repoName, cloneUrl, region } = cfg;

  const stsClient     = new STSClient({ region });
  const secretsClient = new SecretsManagerClient({ region });
  const snsClient     = new SNSClient({ region });

  // 1. Get account ID
  logger.info('Getting AWS account information...');
  const accountId = await getAwsAccountId(stsClient);
  logger.info(`AWS Account ID: ${accountId}`);

  // 2. Retrieve and verify secret from Secrets Manager
  logger.info('Retrieving configuration from Secrets Manager...');
  const secretData = await retrieveSecret(secretsClient, secretName);
  const verifiedRepoName = (secretData['repository_name'] as string) ?? repoName;
  logger.info(`Verified configuration for repository: ${verifiedRepoName}`);

  // 3. Send notification via SNS
  logger.info('Sending repository notification via SNS...');
  await sendNotification(snsClient, topicArn, repoName, cloneUrl);

  return {
    accountId,
    topicArn,
    secretName,
    repoName,
    cloneUrl,
    region,
  };
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

  logger.info('Starting Secure Repository Monitoring...');
  logger.info(`Using SNS topic:  ${cfg.topicArn}`);
  logger.info(`Using secret:     ${cfg.secretName}`);
  logger.info(`Using repo:       ${cfg.repoName}`);
  logger.info(`Using region:     ${cfg.region}`);

  try {
    const result = await runSecureRepoMonitoring(cfg);

    logger.info('='.repeat(60));
    logger.info('APPLICATION COMPLETED SUCCESSFULLY!');
    logger.info('='.repeat(60));
    logger.info('Resources used:');
    logger.info(`  - SNS Topic:  ${result.topicArn}`);
    logger.info(`  - Secret:     ${result.secretName}`);
    logger.info(`  - Repo:       ${result.repoName}`);
    logger.info(`  - Clone URL:  ${result.cloneUrl}`);
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
