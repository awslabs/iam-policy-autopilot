// Package main — entry point for the metrics aggregation service.
package main

import (
	"context"
	"log/slog"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/aws/aws-sdk-go-v2/config"
)

func main() {
	// Structured logging.
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{
		Level: slog.LevelInfo,
	})))

	// Load configuration.
	cfg, err := LoadConfig()
	if err != nil {
		slog.Error("Failed to load config", "err", err)
		os.Exit(1)
	}

	// Build AWS config.
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	awsCfg, err := config.LoadDefaultConfig(ctx,
		config.WithRegion(cfg.KinesisRegion),
	)
	if err != nil {
		slog.Error("Failed to load AWS config", "err", err)
		os.Exit(1)
	}

	// Optionally load remote feature flags from AppConfig.
	if _, err := LoadRemoteConfig(ctx, awsCfg, cfg); err != nil {
		slog.Warn("Could not load AppConfig — using defaults", "err", err)
	}

	// Wire up components.
	consumer := NewKinesisConsumer(awsCfg, cfg)
	aggregator := NewAggregator(cfg.WindowDuration)
	writer := NewDynamoWriter(awsCfg, cfg)

	recordCh := make(chan KinesisRecord, 1000)

	// Graceful shutdown on SIGTERM / SIGINT.
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGTERM, syscall.SIGINT)
	go func() {
		sig := <-sigCh
		slog.Info("Received signal — shutting down", "signal", sig)
		cancel()
	}()

	// Start Kinesis consumer in background.
	go func() {
		if err := consumer.Run(ctx, recordCh); err != nil && ctx.Err() == nil {
			slog.Error("Kinesis consumer error", "err", err)
			cancel()
		}
	}()

	// Flush ticker — emit completed windows every push interval.
	flushTicker := time.NewTicker(cfg.PushInterval)
	defer flushTicker.Stop()

	slog.Info("Metrics aggregation service started",
		"stream", cfg.KinesisStreamName,
		"table", cfg.MetricsTable,
		"window", cfg.WindowDuration,
	)

	for {
		select {
		case <-ctx.Done():
			slog.Info("Context cancelled — flushing remaining windows")
			finalMetrics := aggregator.Flush(time.Now().Add(cfg.WindowDuration))
			if len(finalMetrics) > 0 {
				if err := writer.WriteBatch(context.Background(), finalMetrics); err != nil {
					slog.Error("Final flush failed", "err", err)
				}
			}
			slog.Info("Shutdown complete")
			return

		case rec := <-recordCh:
			if err := aggregator.Add(rec.Data); err != nil {
				slog.Warn("Failed to add record to aggregator", "err", err, "partition_key", rec.PartitionKey)
			}

		case <-flushTicker.C:
			cutoff := time.Now().Add(-cfg.WindowDuration)
			metrics := aggregator.Flush(cutoff)
			if len(metrics) == 0 {
				continue
			}
			flushCtx, flushCancel := context.WithTimeout(ctx, 30*time.Second)
			if err := writer.WriteBatch(flushCtx, metrics); err != nil {
				slog.Error("Failed to write metrics batch", "err", err, "count", len(metrics))
			}
			flushCancel()
		}
	}
}
