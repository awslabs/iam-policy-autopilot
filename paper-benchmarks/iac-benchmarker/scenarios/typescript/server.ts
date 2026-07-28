/**
 * Express HTTP server for the Feature Flag Service.
 *
 * Endpoints:
 *   GET  /flags                    — list all flags
 *   GET  /flags/:key               — get a single flag definition
 *   POST /flags/:key               — create or update a flag
 *   DELETE /flags/:key             — delete a flag
 *   POST /evaluate                 — evaluate a flag for a user context
 *   GET  /health                   — health check
 */

import express, { Request, Response, NextFunction } from 'express';
import { loadConfig, loadAppConfig } from './config';
import { DynamoFlagClient, FeatureFlag } from './dynamodb-client';
import { KinesisPublisher } from './kinesis-publisher';
import { FlagEvaluator, EvaluationContext } from './flag-evaluator';

async function main(): Promise<void> {
  const cfg = await loadConfig();

  // Load remote feature toggles from AppConfig.
  const remoteToggles = await loadAppConfig(cfg).catch(err => {
    console.warn('[AppConfig] Could not load remote config:', err);
    return {};
  });
  console.info('[AppConfig] Loaded toggles:', Object.keys(remoteToggles));

  // Wire up components.
  const dynamo   = new DynamoFlagClient(cfg.dynamoRegion, cfg.featureFlagsTable);
  const kinesis  = new KinesisPublisher(cfg.kinesisRegion, cfg.kinesisStreamName, cfg.kinesisBatchSize);
  const evaluator = new FlagEvaluator(dynamo, kinesis, cfg.cacheTtlSeconds);

  kinesis.startAutoFlush(5_000);

  const app = express();
  app.use(express.json());

  // ── Routes ──────────────────────────────────────────────────────────────

  app.get('/health', (_req: Request, res: Response) => {
    res.json({ status: 'ok', timestamp: new Date().toISOString() });
  });

  app.get('/flags', async (_req: Request, res: Response, next: NextFunction) => {
    try {
      const flags = await dynamo.listFlags();
      res.json({ flags });
    } catch (err) { next(err); }
  });

  app.get('/flags/:key', async (req: Request, res: Response, next: NextFunction) => {
    try {
      const flag = await dynamo.getFlag(req.params['key']!);
      if (!flag) return res.status(404).json({ error: 'Flag not found' });
      res.json(flag);
    } catch (err) { next(err); }
  });

  app.post('/flags/:key', async (req: Request, res: Response, next: NextFunction) => {
    try {
      const flag: FeatureFlag = {
        flagKey:        req.params['key']!,
        enabled:        req.body.enabled ?? false,
        rolloutPct:     req.body.rolloutPct ?? 100,
        targetingRules: req.body.targetingRules ?? [],
        description:    req.body.description ?? '',
        updatedAt:      new Date().toISOString(),
        updatedBy:      req.body.updatedBy ?? 'api',
      };
      await dynamo.putFlag(flag);
      evaluator.invalidate(flag.flagKey);
      res.status(201).json(flag);
    } catch (err) { next(err); }
  });

  app.delete('/flags/:key', async (req: Request, res: Response, next: NextFunction) => {
    try {
      await dynamo.deleteFlag(req.params['key']!);
      evaluator.invalidate(req.params['key']!);
      res.status(204).send();
    } catch (err) { next(err); }
  });

  app.post('/evaluate', async (req: Request, res: Response, next: NextFunction) => {
    try {
      const { flagKey, userId, attributes } = req.body as {
        flagKey: string;
        userId: string;
        attributes?: Record<string, string>;
      };
      if (!flagKey || !userId) {
        return res.status(400).json({ error: 'flagKey and userId are required' });
      }
      const ctx: EvaluationContext = { userId, attributes };
      const result = await evaluator.evaluate(flagKey, ctx);
      res.json(result);
    } catch (err) { next(err); }
  });

  // ── Error handler ────────────────────────────────────────────────────────

  app.use((err: Error, _req: Request, res: Response, _next: NextFunction) => {
    console.error('[Server] Unhandled error:', err);
    res.status(500).json({ error: err.message });
  });

  // ── Graceful shutdown ────────────────────────────────────────────────────

  const server = app.listen(cfg.port, () => {
    console.info(`[Server] Feature Flag Service listening on port ${cfg.port}`);
  });

  const shutdown = async () => {
    console.info('[Server] Shutting down ...');
    await kinesis.shutdown();
    server.close(() => process.exit(0));
  };

  process.on('SIGTERM', shutdown);
  process.on('SIGINT', shutdown);
}

main().catch(err => {
  console.error('[Server] Fatal error:', err);
  process.exit(1);
});
