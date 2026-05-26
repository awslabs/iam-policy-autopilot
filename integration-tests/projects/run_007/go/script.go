package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"math/rand"
	"os"
	"path/filepath"
	"runtime"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"github.com/aws/aws-sdk-go-v2/service/ses"
	sestypes "github.com/aws/aws-sdk-go-v2/service/ses/types"
	"github.com/aws/aws-sdk-go-v2/service/sts"
	"github.com/aws/smithy-go"
)

// ── Config ────────────────────────────────────────────────────────────────────

// RunConfig mirrors the shape of config.json written by deploy.sh.
type RunConfig struct {
	BucketName string `json:"bucketName"`
	Region     string `json:"region"`
}

func loadConfig() (*RunConfig, error) {
	// Locate config.json one directory above this source file.
	_, filename, _, _ := runtime.Caller(0)
	configPath := filepath.Join(filepath.Dir(filename), "..", "config.json")
	configPath = filepath.Clean(configPath)

	data, err := os.ReadFile(configPath)
	if err != nil {
		return nil, fmt.Errorf(
			"config.json not found at %s — deploy the CDK stack first:\n  cd ../cdk && bash deploy.sh",
			configPath,
		)
	}

	var cfg RunConfig
	if err := json.Unmarshal(data, &cfg); err != nil {
		return nil, fmt.Errorf("failed to parse config.json: %w", err)
	}
	return &cfg, nil
}

// ── Data types ────────────────────────────────────────────────────────────────

type MonitoringData struct {
	Timestamp          float64            `json:"timestamp"`
	SystemMetrics      SystemMetrics      `json:"system_metrics"`
	ApplicationMetrics ApplicationMetrics `json:"application_metrics"`
}

type SystemMetrics struct {
	CPUUsage    float64 `json:"cpu_usage"`
	MemoryUsage float64 `json:"memory_usage"`
	DiskUsage   float64 `json:"disk_usage"`
}

type ApplicationMetrics struct {
	RequestsPerSecond float64 `json:"requests_per_second"`
	ErrorRate         float64 `json:"error_rate"`
	ResponseTime      float64 `json:"response_time"`
}

// ── Helpers ───────────────────────────────────────────────────────────────────

func uploadSampleData(ctx context.Context, s3Client *s3.Client, bucketName string) error {
	sampleData := MonitoringData{
		Timestamp: float64(time.Now().Unix()),
		SystemMetrics: SystemMetrics{
			CPUUsage:    rand.Float64()*80 + 10,  // 10-90
			MemoryUsage: rand.Float64()*60 + 20,  // 20-80
			DiskUsage:   rand.Float64()*40 + 30,  // 30-70
		},
		ApplicationMetrics: ApplicationMetrics{
			RequestsPerSecond: float64(rand.Intn(900) + 100), // 100-1000
			ErrorRate:         rand.Float64()*4.9 + 0.1,      // 0.1-5.0
			ResponseTime:      rand.Float64()*400 + 100,      // 100-500
		},
	}

	dataJSON, err := json.MarshalIndent(sampleData, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal sample data: %w", err)
	}

	keyName := fmt.Sprintf("monitoring-data/%d.json", time.Now().Unix())
	_, err = s3Client.PutObject(ctx, &s3.PutObjectInput{
		Bucket:      aws.String(bucketName),
		Key:         aws.String(keyName),
		Body:        bytes.NewReader(dataJSON),
		ContentType: aws.String("application/json"),
	})
	if err != nil {
		return fmt.Errorf("failed to upload sample data: %w", err)
	}

	log.Printf("Uploaded sample monitoring data to S3: s3://%s/%s", bucketName, keyName)
	return nil
}

func sendNotificationEmail(ctx context.Context, sesClient *ses.Client, senderEmail, recipientEmail string) error {
	// Only tolerate MessageRejected (unverified addresses in sandbox).
	// All other errors — including AccessDeniedException — propagate so the
	// minimizer can detect missing permissions.
	subject := "AWS Monitoring System - Setup Complete"
	body := "Your AWS monitoring system has been successfully set up.\n\nThe system is now ready for use."

	_, err := sesClient.SendEmail(ctx, &ses.SendEmailInput{
		Source: aws.String(senderEmail),
		Destination: &sestypes.Destination{
			ToAddresses: []string{recipientEmail},
		},
		Message: &sestypes.Message{
			Subject: &sestypes.Content{
				Data: aws.String(subject),
			},
			Body: &sestypes.Body{
				Text: &sestypes.Content{
					Data: aws.String(body),
				},
			},
		},
	})
	if err != nil {
		var apiErr smithy.APIError
		if errors.As(err, &apiErr) && apiErr.ErrorCode() == "MessageRejected" {
			log.Printf("Warning: SES SendEmail: email not verified (non-fatal)")
			return nil
		}
		return fmt.Errorf("ses SendEmail failed: %w", err)
	}

	log.Printf("Sent notification email to %s", recipientEmail)
	return nil
}

// ── Main logic ────────────────────────────────────────────────────────────────

func run(ctx context.Context, cfg *RunConfig) error {
	log.Printf("Starting AWS Comprehensive Monitoring System (data-plane)...")
	log.Printf("Using bucket: %s", cfg.BucketName)
	log.Printf("Using region: %s", cfg.Region)

	awsCfg, err := config.LoadDefaultConfig(ctx, config.WithRegion(cfg.Region))
	if err != nil {
		return fmt.Errorf("failed to load AWS config: %w", err)
	}

	stsClient := sts.NewFromConfig(awsCfg)
	s3Client  := s3.NewFromConfig(awsCfg)
	sesClient := ses.NewFromConfig(awsCfg)

	// STS GetCallerIdentity — verify credentials
	identity, err := stsClient.GetCallerIdentity(ctx, &sts.GetCallerIdentityInput{})
	if err != nil {
		return fmt.Errorf("failed to get caller identity: %w", err)
	}
	log.Printf("AWS Account ID: %s", aws.ToString(identity.Account))

	// S3 PutObject — upload monitoring data
	if err := uploadSampleData(ctx, s3Client, cfg.BucketName); err != nil {
		return err
	}

	// SES SendEmail — MessageRejected is tolerated; permission errors propagate
	if err := sendNotificationEmail(ctx, sesClient, "test@example.com", "test@example.com"); err != nil {
		return err
	}

	log.Printf("AWS Comprehensive Monitoring System completed successfully!")
	log.Printf("  - S3 Bucket: %s", cfg.BucketName)
	return nil
}

// ── Entry point ───────────────────────────────────────────────────────────────

func main() {
	log.SetFlags(log.LstdFlags)
	log.SetPrefix("[ComprehensiveMonitoring] ")

	ctx := context.Background()

	cfg, err := loadConfig()
	if err != nil {
		log.Fatalf("Configuration error: %v", err)
	}

	if err := run(ctx, cfg); err != nil {
		log.Fatalf("Application failed: %v", err)
	}
}
