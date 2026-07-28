/**
 * DynamoDB client for feature flag definitions.
 *
 * The `feature-flags` table schema:
 *   PK: flag_key (String)
 *   Attributes: enabled (Boolean), rollout_pct (Number), targeting_rules (String/JSON),
 *               description (String), updated_at (String/ISO-8601), updated_by (String)
 */

import {
  DynamoDBClient,
  GetItemCommand,
  PutItemCommand,
  DeleteItemCommand,
  ScanCommand,
  UpdateItemCommand,
} from '@aws-sdk/client-dynamodb';
import { marshall, unmarshall } from '@aws-sdk/util-dynamodb';

export interface FeatureFlag {
  flagKey: string;
  enabled: boolean;
  rolloutPct: number;          // 0–100
  targetingRules: TargetingRule[];
  description: string;
  updatedAt: string;
  updatedBy: string;
}

export interface TargetingRule {
  attribute: string;           // e.g. "user_id", "country", "plan"
  operator: 'eq' | 'in' | 'prefix';
  values: string[];
  result: boolean;             // override value when rule matches
}

export class DynamoFlagClient {
  private readonly client: DynamoDBClient;
  private readonly tableName: string;

  constructor(region: string, tableName: string) {
    this.client    = new DynamoDBClient({ region });
    this.tableName = tableName;
  }

  async getFlag(flagKey: string): Promise<FeatureFlag | null> {
    const resp = await this.client.send(new GetItemCommand({
      TableName: this.tableName,
      Key: marshall({ flag_key: flagKey }),
    }));
    if (!resp.Item) return null;
    return this.fromDynamo(unmarshall(resp.Item));
  }

  async putFlag(flag: FeatureFlag): Promise<void> {
    await this.client.send(new PutItemCommand({
      TableName: this.tableName,
      Item: marshall(this.toDynamo(flag)),
    }));
  }

  async deleteFlag(flagKey: string): Promise<void> {
    await this.client.send(new DeleteItemCommand({
      TableName: this.tableName,
      Key: marshall({ flag_key: flagKey }),
    }));
  }

  async listFlags(): Promise<FeatureFlag[]> {
    const flags: FeatureFlag[] = [];
    let lastKey: Record<string, unknown> | undefined;

    do {
      const resp = await this.client.send(new ScanCommand({
        TableName: this.tableName,
        ExclusiveStartKey: lastKey ? marshall(lastKey) : undefined,
      }));
      for (const item of resp.Items ?? []) {
        flags.push(this.fromDynamo(unmarshall(item)));
      }
      lastKey = resp.LastEvaluatedKey ? unmarshall(resp.LastEvaluatedKey) : undefined;
    } while (lastKey);

    return flags;
  }

  async setEnabled(flagKey: string, enabled: boolean, updatedBy: string): Promise<void> {
    await this.client.send(new UpdateItemCommand({
      TableName: this.tableName,
      Key: marshall({ flag_key: flagKey }),
      UpdateExpression: 'SET enabled = :e, updated_at = :ts, updated_by = :by',
      ExpressionAttributeValues: marshall({
        ':e':  enabled,
        ':ts': new Date().toISOString(),
        ':by': updatedBy,
      }),
    }));
  }

  // ── serialisation helpers ─────────────────────────────────────────────────

  private toDynamo(flag: FeatureFlag): Record<string, unknown> {
    return {
      flag_key:        flag.flagKey,
      enabled:         flag.enabled,
      rollout_pct:     flag.rolloutPct,
      targeting_rules: JSON.stringify(flag.targetingRules),
      description:     flag.description,
      updated_at:      flag.updatedAt,
      updated_by:      flag.updatedBy,
    };
  }

  private fromDynamo(item: Record<string, unknown>): FeatureFlag {
    return {
      flagKey:        item['flag_key'] as string,
      enabled:        item['enabled'] as boolean,
      rolloutPct:     (item['rollout_pct'] as number) ?? 0,
      targetingRules: JSON.parse((item['targeting_rules'] as string) ?? '[]') as TargetingRule[],
      description:    (item['description'] as string) ?? '',
      updatedAt:      (item['updated_at'] as string) ?? '',
      updatedBy:      (item['updated_by'] as string) ?? '',
    };
  }
}
