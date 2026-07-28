// Package main — configuration for the metrics aggregation service.
package main

import (
	"context"
	"fmt"
	"os"
	"strconv"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/service/appconfigdata"
)

// Config holds all runtime configuration for the metrics aggregation service.
type Config struct {
	// Kinesis
	KinesisStreamName string
	KinesisRegion     string
	ShardPollInterval time.Duration
	MaxRecordsPerCall int32

	// DynamoDB
	MetricsTable  string
	DynamoRegion  string
	WriteBatchSize int

	// S3 / Firehose
	ArchiveBucket        string
	FirehoseDeliveryName string

	// CloudWatch
	MetricNamespace string
	PushInterval    time.Duration

	// AppConfig (optional)
	AppConfigApp         string
	AppConfigEnv         string
	AppConfigProfile     string
	AppConfigPollSeconds int

	// Aggregation window
	WindowDuration time.Duration
}

// LoadConfig reads configuration from environment variables.
func LoadConfig() (*Config, error) {
	cfg := &Config{
		KinesisStreamName:    requireEnv("KINESIS_STREAM_NAME"),
		KinesisRegion:        getEnv("AWS_REGION", "us-east-1"),
		ShardPollInterval:    parseDuration("SHARD_POLL_INTERVAL", 1*time.Second),
		MaxRecordsPerCall:    int32(parseInt("MAX_RECORDS_PER_CALL", 100)),
		MetricsTable:         getEnv("METRICS_TABLE", "metrics-aggregates"),
		DynamoRegion:         getEnv("AWS_REGION", "us-east-1"),
		WriteBatchSize:       parseInt("WRITE_BATCH_SIZE", 25),
		ArchiveBucket:        requireEnv("ARCHIVE_BUCKET"),
		FirehoseDeliveryName: getEnv("FIREHOSE_DELIVERY_NAME", "metrics-firehose"),
		MetricNamespace:      getEnv("METRIC_NAMESPACE", "MetricsAggregationService"),
		PushInterval:         parseDuration("PUSH_INTERVAL", 60*time.Second),
		AppConfigApp:         getEnv("APPCONFIG_APP", ""),
		AppConfigEnv:         getEnv("APPCONFIG_ENV", ""),
		AppConfigProfile:     getEnv("APPCONFIG_PROFILE", ""),
		AppConfigPollSeconds: parseInt("APPCONFIG_POLL_SECONDS", 60),
		WindowDuration:       parseDuration("WINDOW_DURATION", 60*time.Second),
	}
	return cfg, nil
}

// LoadRemoteConfig fetches feature flags from AWS AppConfig (optional).
// Returns the raw configuration bytes, or nil if AppConfig is not configured.
func LoadRemoteConfig(ctx context.Context, awsCfg aws.Config, cfg *Config) ([]byte, error) {
	if cfg.AppConfigApp == "" || cfg.AppConfigEnv == "" || cfg.AppConfigProfile == "" {
		return nil, nil
	}

	client := appconfigdata.NewFromConfig(awsCfg)

	startResp, err := client.StartConfigurationSession(ctx, &appconfigdata.StartConfigurationSessionInput{
		ApplicationIdentifier:          aws.String(cfg.AppConfigApp),
		EnvironmentIdentifier:          aws.String(cfg.AppConfigEnv),
		ConfigurationProfileIdentifier: aws.String(cfg.AppConfigProfile),
		RequiredMinimumPollIntervalInSeconds: aws.Int32(int32(cfg.AppConfigPollSeconds)),
	})
	if err != nil {
		return nil, fmt.Errorf("start AppConfig session: %w", err)
	}

	getResp, err := client.GetLatestConfiguration(ctx, &appconfigdata.GetLatestConfigurationInput{
		ConfigurationToken: startResp.InitialConfigurationToken,
	})
	if err != nil {
		return nil, fmt.Errorf("get AppConfig configuration: %w", err)
	}

	return getResp.Configuration, nil
}

// ── helpers ──────────────────────────────────────────────────────────────────

func requireEnv(name string) string {
	v := os.Getenv(name)
	if v == "" {
		panic(fmt.Sprintf("required environment variable %q is not set", name))
	}
	return v
}

func getEnv(name, fallback string) string {
	if v := os.Getenv(name); v != "" {
		return v
	}
	return fallback
}

func parseInt(name string, fallback int) int {
	if v := os.Getenv(name); v != "" {
		if n, err := strconv.Atoi(v); err == nil {
			return n
		}
	}
	return fallback
}

func parseDuration(name string, fallback time.Duration) time.Duration {
	if v := os.Getenv(name); v != "" {
		if d, err := time.ParseDuration(v); err == nil {
			return d
		}
	}
	return fallback
}
