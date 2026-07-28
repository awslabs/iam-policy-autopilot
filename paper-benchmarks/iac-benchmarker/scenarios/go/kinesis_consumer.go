// Package main — Kinesis shard consumer for the metrics aggregation service.
package main

import (
	"context"
	"fmt"
	"log/slog"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/service/kinesis"
	"github.com/aws/aws-sdk-go-v2/service/kinesis/types"
)

// KinesisRecord is a decoded metric record from the Kinesis stream.
type KinesisRecord struct {
	PartitionKey   string
	SequenceNumber string
	Data           []byte
	ApproximateArrivalTimestamp time.Time
}

// KinesisConsumer polls all shards of a Kinesis stream and emits records.
type KinesisConsumer struct {
	client     *kinesis.Client
	streamName string
	pollInterval time.Duration
	maxRecords int32
}

// NewKinesisConsumer creates a new consumer for the given stream.
func NewKinesisConsumer(awsCfg aws.Config, cfg *Config) *KinesisConsumer {
	return &KinesisConsumer{
		client:       kinesis.NewFromConfig(awsCfg),
		streamName:   cfg.KinesisStreamName,
		pollInterval: cfg.ShardPollInterval,
		maxRecords:   cfg.MaxRecordsPerCall,
	}
}

// Run starts consuming all shards and sends records to the out channel.
// Blocks until ctx is cancelled.
func (c *KinesisConsumer) Run(ctx context.Context, out chan<- KinesisRecord) error {
	shards, err := c.listShards(ctx)
	if err != nil {
		return fmt.Errorf("list shards: %w", err)
	}
	slog.Info("Starting Kinesis consumer", "stream", c.streamName, "shards", len(shards))

	errCh := make(chan error, len(shards))
	for _, shard := range shards {
		go func(shardID string) {
			errCh <- c.consumeShard(ctx, shardID, out)
		}(*shard.ShardId)
	}

	// Wait for first error or context cancellation.
	select {
	case err := <-errCh:
		return err
	case <-ctx.Done():
		return ctx.Err()
	}
}

// consumeShard polls a single shard from TRIM_HORIZON and forwards records.
func (c *KinesisConsumer) consumeShard(ctx context.Context, shardID string, out chan<- KinesisRecord) error {
	iterResp, err := c.client.GetShardIterator(ctx, &kinesis.GetShardIteratorInput{
		StreamName:        aws.String(c.streamName),
		ShardId:           aws.String(shardID),
		ShardIteratorType: types.ShardIteratorTypeTrimHorizon,
	})
	if err != nil {
		return fmt.Errorf("get shard iterator for %s: %w", shardID, err)
	}

	iterator := iterResp.ShardIterator
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		default:
		}

		resp, err := c.client.GetRecords(ctx, &kinesis.GetRecordsInput{
			ShardIterator: iterator,
			Limit:         aws.Int32(c.maxRecords),
		})
		if err != nil {
			slog.Error("GetRecords failed", "shard", shardID, "err", err)
			time.Sleep(c.pollInterval)
			continue
		}

		for _, r := range resp.Records {
			out <- KinesisRecord{
				PartitionKey:   aws.ToString(r.PartitionKey),
				SequenceNumber: aws.ToString(r.SequenceNumber),
				Data:           r.Data,
				ApproximateArrivalTimestamp: aws.ToTime(r.ApproximateArrivalTimestamp),
			}
		}

		if resp.NextShardIterator == nil {
			slog.Info("Shard exhausted", "shard", shardID)
			return nil
		}
		iterator = resp.NextShardIterator

		if len(resp.Records) == 0 {
			time.Sleep(c.pollInterval)
		}
	}
}

// listShards returns all active shards for the configured stream.
func (c *KinesisConsumer) listShards(ctx context.Context) ([]types.Shard, error) {
	var shards []types.Shard
	var nextToken *string

	for {
		input := &kinesis.ListShardsInput{
			StreamName: aws.String(c.streamName),
		}
		if nextToken != nil {
			input.StreamName = nil
			input.NextToken = nextToken
		}

		resp, err := c.client.ListShards(ctx, input)
		if err != nil {
			return nil, err
		}
		shards = append(shards, resp.Shards...)
		if resp.NextToken == nil {
			break
		}
		nextToken = resp.NextToken
	}
	return shards, nil
}
