// Package main — DynamoDB batch writer for aggregated metrics.
package main

import (
	"context"
	"fmt"
	"log/slog"
	"strconv"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/feature/dynamodb/attributevalue"
	"github.com/aws/aws-sdk-go-v2/service/dynamodb"
	"github.com/aws/aws-sdk-go-v2/service/dynamodb/types"
)

// dynamoItem is the DynamoDB representation of an AggregatedMetric.
type dynamoItem struct {
	MetricName  string  `dynamodbav:"metric_name"`
	WindowStart string  `dynamodbav:"window_start"` // ISO-8601
	WindowEnd   string  `dynamodbav:"window_end"`
	Count       int64   `dynamodbav:"count"`
	Sum         float64 `dynamodbav:"sum"`
	Min         float64 `dynamodbav:"min"`
	Max         float64 `dynamodbav:"max"`
	Avg         float64 `dynamodbav:"avg"`
	P50         float64 `dynamodbav:"p50"`
	P95         float64 `dynamodbav:"p95"`
	P99         float64 `dynamodbav:"p99"`
	TTL         int64   `dynamodbav:"ttl"` // Unix epoch — items expire after 90 days
}

// DynamoWriter writes AggregatedMetrics to DynamoDB in batches.
type DynamoWriter struct {
	client    *dynamodb.Client
	tableName string
	batchSize int
}

// NewDynamoWriter creates a writer for the given table.
func NewDynamoWriter(awsCfg aws.Config, cfg *Config) *DynamoWriter {
	return &DynamoWriter{
		client:    dynamodb.NewFromConfig(awsCfg),
		tableName: cfg.MetricsTable,
		batchSize: cfg.WriteBatchSize,
	}
}

// WriteBatch writes a slice of AggregatedMetrics to DynamoDB.
// Items are split into batches of up to 25 (DynamoDB limit).
func (w *DynamoWriter) WriteBatch(ctx context.Context, metrics []AggregatedMetric) error {
	if len(metrics) == 0 {
		return nil
	}

	ttlCutoff := time.Now().Add(90 * 24 * time.Hour).Unix()

	var requests []types.WriteRequest
	for _, m := range metrics {
		item := dynamoItem{
			MetricName:  m.MetricName,
			WindowStart: m.WindowStart.UTC().Format(time.RFC3339),
			WindowEnd:   m.WindowEnd.UTC().Format(time.RFC3339),
			Count:       m.Count,
			Sum:         m.Sum,
			Min:         m.Min,
			Max:         m.Max,
			Avg:         m.Avg,
			P50:         m.P50,
			P95:         m.P95,
			P99:         m.P99,
			TTL:         ttlCutoff,
		}
		av, err := attributevalue.MarshalMap(item)
		if err != nil {
			return fmt.Errorf("marshal metric %s: %w", m.MetricName, err)
		}
		requests = append(requests, types.WriteRequest{
			PutRequest: &types.PutRequest{Item: av},
		})
	}

	// Split into batches of batchSize.
	for i := 0; i < len(requests); i += w.batchSize {
		end := i + w.batchSize
		if end > len(requests) {
			end = len(requests)
		}
		batch := requests[i:end]

		if err := w.sendBatch(ctx, batch); err != nil {
			return fmt.Errorf("send batch [%d:%d]: %w", i, end, err)
		}
	}

	slog.Info("Wrote metrics to DynamoDB", "count", len(metrics), "table", w.tableName)
	return nil
}

// sendBatch sends one DynamoDB BatchWriteItem request, retrying unprocessed items.
func (w *DynamoWriter) sendBatch(ctx context.Context, requests []types.WriteRequest) error {
	unprocessed := map[string][]types.WriteRequest{w.tableName: requests}

	for attempt := 0; len(unprocessed) > 0 && attempt < 5; attempt++ {
		if attempt > 0 {
			time.Sleep(time.Duration(1<<attempt) * 100 * time.Millisecond)
		}
		resp, err := w.client.BatchWriteItem(ctx, &dynamodb.BatchWriteItemInput{
			RequestItems: unprocessed,
		})
		if err != nil {
			return fmt.Errorf("BatchWriteItem: %w", err)
		}
		unprocessed = resp.UnprocessedItems
	}

	if len(unprocessed) > 0 {
		total := 0
		for _, reqs := range unprocessed {
			total += len(reqs)
		}
		return fmt.Errorf("%d items still unprocessed after retries", total)
	}
	return nil
}

// QueryWindow retrieves all aggregated metrics for a given metric name and time range.
func (w *DynamoWriter) QueryWindow(
	ctx context.Context,
	metricName string,
	from, to time.Time,
) ([]dynamoItem, error) {
	fromStr := from.UTC().Format(time.RFC3339)
	toStr := to.UTC().Format(time.RFC3339)

	resp, err := w.client.Query(ctx, &dynamodb.QueryInput{
		TableName:              aws.String(w.tableName),
		KeyConditionExpression: aws.String("metric_name = :mn AND window_start BETWEEN :from AND :to"),
		ExpressionAttributeValues: map[string]types.AttributeValue{
			":mn":   &types.AttributeValueMemberS{Value: metricName},
			":from": &types.AttributeValueMemberS{Value: fromStr},
			":to":   &types.AttributeValueMemberS{Value: toStr},
		},
	})
	if err != nil {
		return nil, fmt.Errorf("query metrics: %w", err)
	}

	var items []dynamoItem
	if err := attributevalue.UnmarshalListOfMaps(resp.Items, &items); err != nil {
		return nil, fmt.Errorf("unmarshal items: %w", err)
	}
	_ = strconv.Itoa // keep import used
	return items, nil
}
