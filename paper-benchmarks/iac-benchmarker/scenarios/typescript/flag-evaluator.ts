/**
 * Core feature flag evaluation logic.
 *
 * Evaluates a flag for a given user context, applying targeting rules
 * first, then percentage rollout, then the global enabled/disabled state.
 * Results are cached in Redis and published to Kinesis.
 */

import { createHash } from 'crypto';
import { DynamoFlagClient, FeatureFlag, TargetingRule } from './dynamodb-client';
import { KinesisPublisher, FlagEvaluationEvent } from './kinesis-publisher';

export interface EvaluationContext {
  userId: string;
  attributes?: Record<string, string>;
}

export interface EvaluationResult {
  flagKey: string;
  value: boolean;
  reason: FlagEvaluationEvent['reason'];
}

/** Simple in-process cache entry. */
interface CacheEntry {
  flag: FeatureFlag;
  expiresAt: number;
}

export class FlagEvaluator {
  private readonly dynamo: DynamoFlagClient;
  private readonly kinesis: KinesisPublisher;
  private readonly cacheTtlMs: number;
  private readonly localCache = new Map<string, CacheEntry>();

  constructor(dynamo: DynamoFlagClient, kinesis: KinesisPublisher, cacheTtlSeconds: number) {
    this.dynamo      = dynamo;
    this.kinesis     = kinesis;
    this.cacheTtlMs  = cacheTtlSeconds * 1_000;
  }

  /** Evaluate a flag for the given user context. */
  async evaluate(flagKey: string, ctx: EvaluationContext): Promise<EvaluationResult> {
    const flag = await this.getFlag(flagKey);

    if (!flag) {
      return this.emit({ flagKey, value: false, reason: 'default' }, ctx);
    }

    if (!flag.enabled) {
      return this.emit({ flagKey, value: false, reason: 'disabled' }, ctx);
    }

    // 1. Check targeting rules.
    for (const rule of flag.targetingRules) {
      if (this.matchesRule(rule, ctx)) {
        return this.emit({ flagKey, value: rule.result, reason: 'targeting' }, ctx);
      }
    }

    // 2. Percentage rollout — deterministic per (flagKey, userId).
    if (flag.rolloutPct < 100) {
      const bucket = this.rolloutBucket(flagKey, ctx.userId);
      if (bucket >= flag.rolloutPct) {
        return this.emit({ flagKey, value: false, reason: 'rollout' }, ctx);
      }
    }

    return this.emit({ flagKey, value: true, reason: 'default' }, ctx);
  }

  // ── internals ─────────────────────────────────────────────────────────────

  private async getFlag(flagKey: string): Promise<FeatureFlag | null> {
    const cached = this.localCache.get(flagKey);
    if (cached && cached.expiresAt > Date.now()) {
      return cached.flag;
    }
    const flag = await this.dynamo.getFlag(flagKey);
    if (flag) {
      this.localCache.set(flagKey, { flag, expiresAt: Date.now() + this.cacheTtlMs });
    }
    return flag;
  }

  private matchesRule(rule: TargetingRule, ctx: EvaluationContext): boolean {
    const attrValue = ctx.attributes?.[rule.attribute];
    if (attrValue === undefined) return false;

    switch (rule.operator) {
      case 'eq':     return rule.values.includes(attrValue);
      case 'in':     return rule.values.includes(attrValue);
      case 'prefix': return rule.values.some(v => attrValue.startsWith(v));
      default:       return false;
    }
  }

  /** Returns a deterministic 0–99 bucket for (flagKey, userId). */
  private rolloutBucket(flagKey: string, userId: string): number {
    const hash = createHash('sha256').update(`${flagKey}:${userId}`).digest('hex');
    return parseInt(hash.slice(0, 8), 16) % 100;
  }

  private async emit(result: EvaluationResult, ctx: EvaluationContext): Promise<EvaluationResult> {
    await this.kinesis.publish({
      flagKey:    result.flagKey,
      userId:     ctx.userId,
      result:     result.value,
      reason:     result.reason,
      timestamp:  new Date().toISOString(),
      attributes: ctx.attributes,
    });
    return result;
  }

  /** Invalidate the local cache for a specific flag. */
  invalidate(flagKey: string): void {
    this.localCache.delete(flagKey);
  }

  /** Clear the entire local cache. */
  clearCache(): void {
    this.localCache.clear();
  }
}
