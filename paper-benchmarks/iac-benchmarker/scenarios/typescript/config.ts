/**
 * Configuration loader for the Feature Flag Service.
 *
 * Reads settings from environment variables, with optional enrichment
 * from AWS Secrets Manager (Redis auth token) and AppConfig (feature toggles).
 */

import {
  SecretsManagerClient,
  GetSecretValueCommand,
} from '@aws-sdk/client-secrets-manager';
import {
  AppConfigDataClient,
  StartConfigurationSessionCommand,
  GetLatestConfigurationCommand,
} from '@aws-sdk/client-appconfigdata';

export interface ServiceConfig {
  // DynamoDB
  featureFlagsTable: string;
  dynamoRegion: string;

  // Kinesis
  kinesisStreamName: string;
  kinesisRegion: string;
  kinesisBatchSize: number;

  // Redis
  redisHost: string;
  redisPort: number;
  redisAuthToken?: string;
  cacheTtlSeconds: number;

  // AppConfig
  appConfigAppId: string;
  appConfigEnvId: string;
  appConfigProfileId: string;
  appConfigPollIntervalSeconds: number;

  // Secrets Manager
  redisSecretArn?: string;

  // HTTP server
  port: number;
  logLevel: string;
}

export async function loadConfig(): Promise<ServiceConfig> {
  const cfg: ServiceConfig = {
    featureFlagsTable:            requireEnv('FEATURE_FLAGS_TABLE'),
    dynamoRegion:                 getEnv('AWS_REGION', 'us-east-1'),
    kinesisStreamName:            requireEnv('KINESIS_STREAM_NAME'),
    kinesisRegion:                getEnv('AWS_REGION', 'us-east-1'),
    kinesisBatchSize:             parseInt(getEnv('KINESIS_BATCH_SIZE', '100'), 10),
    redisHost:                    getEnv('REDIS_HOST', 'localhost'),
    redisPort:                    parseInt(getEnv('REDIS_PORT', '6379'), 10),
    cacheTtlSeconds:              parseInt(getEnv('CACHE_TTL_SECONDS', '60'), 10),
    appConfigAppId:               getEnv('APPCONFIG_APP_ID', ''),
    appConfigEnvId:               getEnv('APPCONFIG_ENV_ID', ''),
    appConfigProfileId:           getEnv('APPCONFIG_PROFILE_ID', ''),
    appConfigPollIntervalSeconds: parseInt(getEnv('APPCONFIG_POLL_INTERVAL_SECONDS', '60'), 10),
    redisSecretArn:               process.env['REDIS_SECRET_ARN'],
    port:                         parseInt(getEnv('PORT', '3000'), 10),
    logLevel:                     getEnv('LOG_LEVEL', 'info'),
  };

  // Enrich with Redis auth token from Secrets Manager.
  if (cfg.redisSecretArn) {
    cfg.redisAuthToken = await loadSecret(cfg.redisSecretArn, cfg.dynamoRegion);
  }

  return cfg;
}

/** Load a secret string from AWS Secrets Manager. */
export async function loadSecret(secretArn: string, region: string): Promise<string> {
  const client = new SecretsManagerClient({ region });
  const resp = await client.send(new GetSecretValueCommand({ SecretId: secretArn }));
  if (!resp.SecretString) {
    throw new Error(`Secret ${secretArn} has no SecretString`);
  }
  return resp.SecretString;
}

/** Fetch the latest configuration from AWS AppConfig. */
export async function loadAppConfig(cfg: ServiceConfig): Promise<Record<string, unknown>> {
  if (!cfg.appConfigAppId || !cfg.appConfigEnvId || !cfg.appConfigProfileId) {
    return {};
  }

  const client = new AppConfigDataClient({ region: cfg.dynamoRegion });

  const session = await client.send(new StartConfigurationSessionCommand({
    ApplicationIdentifier:          cfg.appConfigAppId,
    EnvironmentIdentifier:          cfg.appConfigEnvId,
    ConfigurationProfileIdentifier: cfg.appConfigProfileId,
    RequiredMinimumPollIntervalInSeconds: cfg.appConfigPollIntervalSeconds,
  }));

  const latest = await client.send(new GetLatestConfigurationCommand({
    ConfigurationToken: session.InitialConfigurationToken!,
  }));

  if (!latest.Configuration || latest.Configuration.length === 0) {
    return {};
  }

  const text = Buffer.from(latest.Configuration).toString('utf-8');
  try {
    return JSON.parse(text) as Record<string, unknown>;
  } catch {
    return {};
  }
}

// ── helpers ──────────────────────────────────────────────────────────────────

function requireEnv(name: string): string {
  const v = process.env[name];
  if (!v) throw new Error(`Required environment variable '${name}' is not set.`);
  return v;
}

function getEnv(name: string, fallback: string): string {
  return process.env[name] ?? fallback;
}
