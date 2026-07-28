// Package main — sliding-window metric aggregator.
package main

import (
	"encoding/json"
	"fmt"
	"log/slog"
	"math"
	"sort"
	"sync"
	"time"
)

// MetricPoint is a single data point received from the Kinesis stream.
type MetricPoint struct {
	MetricName string    `json:"metric_name"`
	Value      float64   `json:"value"`
	Timestamp  time.Time `json:"timestamp"`
	Tags       map[string]string `json:"tags,omitempty"`
}

// AggregatedMetric holds the aggregated statistics for one metric in one window.
type AggregatedMetric struct {
	MetricName  string
	WindowStart time.Time
	WindowEnd   time.Time
	Count       int64
	Sum         float64
	Min         float64
	Max         float64
	Avg         float64
	P50         float64
	P95         float64
	P99         float64
}

// windowKey uniquely identifies a (metric, window) pair.
type windowKey struct {
	metricName  string
	windowStart int64 // Unix seconds, truncated to window size
}

// windowBucket accumulates raw values for one window.
type windowBucket struct {
	values []float64
	mu     sync.Mutex
}

func (b *windowBucket) add(v float64) {
	b.mu.Lock()
	b.values = append(b.values, v)
	b.mu.Unlock()
}

func (b *windowBucket) aggregate(name string, start, end time.Time) AggregatedMetric {
	b.mu.Lock()
	vals := make([]float64, len(b.values))
	copy(vals, b.values)
	b.mu.Unlock()

	if len(vals) == 0 {
		return AggregatedMetric{MetricName: name, WindowStart: start, WindowEnd: end}
	}

	sort.Float64s(vals)
	sum := 0.0
	for _, v := range vals {
		sum += v
	}
	return AggregatedMetric{
		MetricName:  name,
		WindowStart: start,
		WindowEnd:   end,
		Count:       int64(len(vals)),
		Sum:         sum,
		Min:         vals[0],
		Max:         vals[len(vals)-1],
		Avg:         sum / float64(len(vals)),
		P50:         percentile(vals, 50),
		P95:         percentile(vals, 95),
		P99:         percentile(vals, 99),
	}
}

// Aggregator accumulates MetricPoints into fixed-duration windows.
type Aggregator struct {
	windowDuration time.Duration
	buckets        map[windowKey]*windowBucket
	mu             sync.Mutex
}

// NewAggregator creates an Aggregator with the given window size.
func NewAggregator(windowDuration time.Duration) *Aggregator {
	return &Aggregator{
		windowDuration: windowDuration,
		buckets:        make(map[windowKey]*windowBucket),
	}
}

// Add decodes a raw Kinesis record and adds it to the appropriate window bucket.
func (a *Aggregator) Add(raw []byte) error {
	var pt MetricPoint
	if err := json.Unmarshal(raw, &pt); err != nil {
		return fmt.Errorf("decode metric point: %w", err)
	}

	windowStart := pt.Timestamp.Truncate(a.windowDuration).Unix()
	key := windowKey{metricName: pt.MetricName, windowStart: windowStart}

	a.mu.Lock()
	bucket, ok := a.buckets[key]
	if !ok {
		bucket = &windowBucket{}
		a.buckets[key] = bucket
	}
	a.mu.Unlock()

	bucket.add(pt.Value)
	return nil
}

// Flush returns all completed windows (those whose end time is before cutoff)
// and removes them from the internal map.
func (a *Aggregator) Flush(cutoff time.Time) []AggregatedMetric {
	a.mu.Lock()
	defer a.mu.Unlock()

	var results []AggregatedMetric
	for key, bucket := range a.buckets {
		windowStart := time.Unix(key.windowStart, 0)
		windowEnd := windowStart.Add(a.windowDuration)
		if windowEnd.Before(cutoff) {
			agg := bucket.aggregate(key.metricName, windowStart, windowEnd)
			results = append(results, agg)
			delete(a.buckets, key)
			slog.Debug("Flushed window", "metric", key.metricName, "start", windowStart, "count", agg.Count)
		}
	}
	return results
}

// ── helpers ──────────────────────────────────────────────────────────────────

func percentile(sorted []float64, p float64) float64 {
	n := len(sorted)
	if n == 0 {
		return math.NaN()
	}
	rank := p / 100.0 * float64(n-1)
	lo := int(math.Floor(rank))
	hi := lo + 1
	if hi >= n {
		return sorted[n-1]
	}
	frac := rank - float64(lo)
	return sorted[lo] + frac*(sorted[hi]-sorted[lo])
}
