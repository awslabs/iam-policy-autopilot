/**
 * Kinesis publisher for feature flag evaluation events.
 *
 * Buffers evaluation records and flushes them in batches using
 * PutRecords (up to 500 records or 5 MB per call).
 */

import {
  KinesisClient,
  PutRecordsCommand,
  PutRecordsRequestEntry,
} from '@aws-sdk/client-kinesis';

export interface FlagEvaluationEvent {
  flagKey: string;
  userId: string;
  result: boolean;
  reason: 'targeting' | 'rollout' | 'default' | 'disabled';
  timestamp: string;
  attributes?: Record<string, string>;
}

export class KinesisPublisher {
  private readonly client: KinesisClient;
  private readonly streamName: string;
  private readonly batchSize: number;
  private buffer: FlagEvaluationEvent[] = [];
  private flushTimer: ReturnType<typeof setInterval> | null = null;

  constructor(region: string, streamName: string, batchSize = 100) {
    this.client     = new KinesisClient({ region });
    this.streamName = streamName;
    this.batchSize  = batchSize;
  }

  /** Add an evaluation event to the buffer. Flushes if batch is full. */
  async publish(event: FlagEvaluationEvent): Promise<void> {
    this.buffer.push(event);
    if (this.buffer.length >= this.batchSize) {
      await this.flush();
    }
  }

  /** Start a periodic flush timer (call once at startup). */
  startAutoFlush(intervalMs = 5_000): void {
    this.flushTimer = setInterval(() => {
      this.flush().catch(err => console.error('[Kinesis] Auto-flush error:', err));
    }, intervalMs);
  }

  /** Stop the auto-flush timer and flush remaining events. */
  async shutdown(): Promise<void> {
    if (this.flushTimer) {
      clearInterval(this.flushTimer);
      this.flushTimer = null;
    }
    await this.flush();
  }

  /** Flush all buffered events to Kinesis. */
  async flush(): Promise<void> {
    if (this.buffer.length === 0) return;

    const batch = this.buffer.splice(0, this.buffer.length);
    const records: PutRecordsRequestEntry[] = batch.map(event => ({
      PartitionKey: event.flagKey,
      Data: Buffer.from(JSON.stringify(event), 'utf-8'),
    }));

    try {
      const resp = await this.client.send(new PutRecordsCommand({
        StreamName: this.streamName,
        Records: records,
      }));

      const failed = resp.FailedRecordCount ?? 0;
      if (failed > 0) {
        console.warn(`[Kinesis] ${failed}/${records.length} records failed`);
        // Re-queue failed records.
        resp.Records?.forEach((r, i) => {
          if (r.ErrorCode) this.buffer.unshift(batch[i]);
        });
      }
    } catch (err) {
      console.error('[Kinesis] PutRecords error:', err);
      // Re-queue all records on transport error.
      this.buffer.unshift(...batch);
    }
  }

  get bufferSize(): number {
    return this.buffer.length;
  }
}
