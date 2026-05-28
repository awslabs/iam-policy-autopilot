package main

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/cloudwatch"
	cwTypes "github.com/aws/aws-sdk-go-v2/service/cloudwatch/types"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"github.com/aws/aws-sdk-go-v2/service/sqs"
	sqsTypes "github.com/aws/aws-sdk-go-v2/service/sqs/types"
	"github.com/aws/aws-sdk-go-v2/service/sts"
)

// ── Config ────────────────────────────────────────────────────────────────────

// RunConfig mirrors the shape of config.json written by deploy.sh.
type RunConfig struct {
	BucketName string `json:"bucketName"`
	QueueURL   string `json:"queueUrl"`
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

// ── Types ─────────────────────────────────────────────────────────────────────

type FileInfo struct {
	Filename string `json:"filename"`
	Size     int64  `json:"size"`
	Type     string `json:"type"`
}

type ProcessedFile struct {
	Filename    string `json:"filename"`
	ProcessedAt string `json:"processed_at"`
	Size        int64  `json:"size"`
	Type        string `json:"type"`
	ProcessedBy string `json:"processed_by"`
}

type SQSMessage struct {
	Action    string `json:"action"`
	Filename  string `json:"filename"`
	Bucket    string `json:"bucket"`
	Size      int64  `json:"size"`
	Timestamp string `json:"timestamp"`
	AccountID string `json:"account_id"`
}

type ProcessingResult struct {
	BucketName     string `json:"bucket_name"`
	QueueURL       string `json:"queue_url"`
	ProcessedFiles int    `json:"processed_files"`
	TotalSize      int64  `json:"total_size"`
}

// ── Logging ───────────────────────────────────────────────────────────────────

func setupLogging() {
	log.SetFlags(log.LstdFlags)
	log.SetPrefix("[FileProcessingMonitor] ")
}

// ── Data-plane helpers ────────────────────────────────────────────────────────

func getAWSAccountID(ctx context.Context, stsClient *sts.Client) (string, error) {
	log.Println("Getting AWS account information...")
	result, err := stsClient.GetCallerIdentity(ctx, &sts.GetCallerIdentityInput{})
	if err != nil {
		return "", fmt.Errorf("failed to get AWS account ID: %w", err)
	}
	accountID := *result.Account
	log.Printf("Using AWS Account ID: %s", accountID)
	return accountID, nil
}

func uploadFileToS3(ctx context.Context, s3Client *s3.Client, bucketName, fileKey, fileContent string) error {
	_, err := s3Client.PutObject(ctx, &s3.PutObjectInput{
		Bucket:      aws.String(bucketName),
		Key:         aws.String(fileKey),
		Body:        strings.NewReader(fileContent),
		ContentType: aws.String("application/json"),
	})
	if err != nil {
		return fmt.Errorf("failed to upload file to S3: %w", err)
	}
	return nil
}

func sendSQSMessage(ctx context.Context, sqsClient *sqs.Client, queueURL string, messageBody SQSMessage) (string, error) {
	messageJSON, err := json.Marshal(messageBody)
	if err != nil {
		return "", fmt.Errorf("failed to marshal message: %w", err)
	}
	result, err := sqsClient.SendMessage(ctx, &sqs.SendMessageInput{
		QueueUrl:    aws.String(queueURL),
		MessageBody: aws.String(string(messageJSON)),
	})
	if err != nil {
		return "", fmt.Errorf("failed to send SQS message: %w", err)
	}
	return *result.MessageId, nil
}

func receiveSQSMessages(ctx context.Context, sqsClient *sqs.Client, queueURL string, maxMessages int32) ([]sqsTypes.Message, error) {
	result, err := sqsClient.ReceiveMessage(ctx, &sqs.ReceiveMessageInput{
		QueueUrl:            aws.String(queueURL),
		MaxNumberOfMessages: maxMessages,
		WaitTimeSeconds:     5,
	})
	if err != nil {
		return nil, fmt.Errorf("failed to receive SQS messages: %w", err)
	}
	return result.Messages, nil
}

func deleteSQSMessage(ctx context.Context, sqsClient *sqs.Client, queueURL, receiptHandle string) error {
	_, err := sqsClient.DeleteMessage(ctx, &sqs.DeleteMessageInput{
		QueueUrl:      aws.String(queueURL),
		ReceiptHandle: aws.String(receiptHandle),
	})
	if err != nil {
		return fmt.Errorf("failed to delete SQS message: %w", err)
	}
	return nil
}

func putCloudWatchMetric(ctx context.Context, cwClient *cloudwatch.Client, namespace, metricName string, value float64, unit cwTypes.StandardUnit) error {
	_, err := cwClient.PutMetricData(ctx, &cloudwatch.PutMetricDataInput{
		Namespace: aws.String(namespace),
		MetricData: []cwTypes.MetricDatum{
			{
				MetricName: aws.String(metricName),
				Value:      aws.Float64(value),
				Unit:       unit,
				Timestamp:  aws.Time(time.Now().UTC()),
			},
		},
	})
	if err != nil {
		return fmt.Errorf("failed to put CloudWatch metric: %w", err)
	}
	return nil
}

// ── Main logic ────────────────────────────────────────────────────────────────

func processFileMonitoringSystem(ctx context.Context, cfg *RunConfig) (*ProcessingResult, error) {
	log.Println("Starting AWS File Processing Monitoring System...")
	log.Printf("Using bucket:    %s", cfg.BucketName)
	log.Printf("Using queue URL: %s", cfg.QueueURL)
	log.Printf("Using region:    %s", cfg.Region)

	awsCfg, err := config.LoadDefaultConfig(ctx, config.WithRegion(cfg.Region))
	if err != nil {
		return nil, fmt.Errorf("failed to load AWS config: %w", err)
	}

	s3Client := s3.NewFromConfig(awsCfg)
	sqsClient := sqs.NewFromConfig(awsCfg)
	cwClient := cloudwatch.NewFromConfig(awsCfg)
	stsClient := sts.NewFromConfig(awsCfg)

	accountID, err := getAWSAccountID(ctx, stsClient)
	if err != nil {
		return nil, err
	}

	filesToProcess := []FileInfo{
		{Filename: "data1.json", Size: 1024, Type: "json"},
		{Filename: "data2.json", Size: 2048, Type: "json"},
		{Filename: "data3.json", Size: 512, Type: "json"},
	}

	processedFiles := 0
	var totalSize int64

	for _, fileInfo := range filesToProcess {
		processedFile := ProcessedFile{
			Filename:    fileInfo.Filename,
			ProcessedAt: time.Now().UTC().Format(time.RFC3339),
			Size:        fileInfo.Size,
			Type:        fileInfo.Type,
			ProcessedBy: "file-monitoring-system",
		}

		fileContentJSON, err := json.MarshalIndent(processedFile, "", "  ")
		if err != nil {
			log.Printf("Error marshaling file content for %s: %v", fileInfo.Filename, err)
			continue
		}

		log.Printf("Uploading %s to S3...", fileInfo.Filename)
		if err := uploadFileToS3(ctx, s3Client, cfg.BucketName, fileInfo.Filename, string(fileContentJSON)); err != nil {
			return nil, fmt.Errorf("error uploading file %s: %w", fileInfo.Filename, err)
		}

		sqsMsg := SQSMessage{
			Action:    "file_processed",
			Filename:  fileInfo.Filename,
			Bucket:    cfg.BucketName,
			Size:      fileInfo.Size,
			Timestamp: time.Now().UTC().Format(time.RFC3339),
			AccountID: accountID,
		}

		log.Printf("Sending processing notification to SQS...")
		messageID, err := sendSQSMessage(ctx, sqsClient, cfg.QueueURL, sqsMsg)
		if err != nil {
			return nil, fmt.Errorf("error sending SQS message for %s: %w", fileInfo.Filename, err)
		}
		log.Printf("SQS message sent with ID: %s", messageID)

		processedFiles++
		totalSize += fileInfo.Size

		log.Printf("Sending metrics to CloudWatch...")
		if err := putCloudWatchMetric(ctx, cwClient, "FileProcessing", "FilesProcessed", 1, cwTypes.StandardUnitCount); err != nil {
			return nil, fmt.Errorf("failed to put FilesProcessed metric: %w", err)
		}
		if err := putCloudWatchMetric(ctx, cwClient, "FileProcessing", "BytesProcessed", float64(fileInfo.Size), cwTypes.StandardUnitBytes); err != nil {
			return nil, fmt.Errorf("failed to put BytesProcessed metric: %w", err)
		}

		time.Sleep(1 * time.Second)
	}

	log.Printf("Reading processing notifications from SQS...")
	messages, err := receiveSQSMessages(ctx, sqsClient, cfg.QueueURL, 10)
	if err != nil {
		return nil, fmt.Errorf("failed to receive SQS messages: %w", err)
	}
	for _, message := range messages {
		var msgBody SQSMessage
		if err := json.Unmarshal([]byte(*message.Body), &msgBody); err != nil {
			return nil, fmt.Errorf("error parsing SQS message: %w", err)
		}
		log.Printf("Processing notification: %s (%d bytes)", msgBody.Filename, msgBody.Size)
		if err := deleteSQSMessage(ctx, sqsClient, cfg.QueueURL, *message.ReceiptHandle); err != nil {
			return nil, fmt.Errorf("failed to delete SQS message: %w", err)
		}
		log.Printf("Notification processed and removed from queue")
	}

	log.Printf("Sending summary metrics to CloudWatch...")
	if err := putCloudWatchMetric(ctx, cwClient, "FileProcessing", "TotalFilesProcessed", float64(processedFiles), cwTypes.StandardUnitCount); err != nil {
		return nil, fmt.Errorf("failed to put TotalFilesProcessed metric: %w", err)
	}
	if err := putCloudWatchMetric(ctx, cwClient, "FileProcessing", "TotalBytesProcessed", float64(totalSize), cwTypes.StandardUnitBytes); err != nil {
		return nil, fmt.Errorf("failed to put TotalBytesProcessed metric: %w", err)
	}

	log.Printf("File processing monitoring completed!")
	log.Printf("Total files processed: %d", processedFiles)
	log.Printf("Total bytes processed: %d", totalSize)
	log.Printf("S3 bucket:   %s", cfg.BucketName)
	log.Printf("SQS queue URL: %s", cfg.QueueURL)

	return &ProcessingResult{
		BucketName:     cfg.BucketName,
		QueueURL:       cfg.QueueURL,
		ProcessedFiles: processedFiles,
		TotalSize:      totalSize,
	}, nil
}

// ── Entry point ───────────────────────────────────────────────────────────────

func main() {
	setupLogging()
	ctx := context.Background()

	cfg, err := loadConfig()
	if err != nil {
		log.Fatalf("Configuration error: %v", err)
	}

	result, err := processFileMonitoringSystem(ctx, cfg)
	if err != nil {
		log.Fatalf("Application failed: %v", err)
	}

	log.Printf(strings.Repeat("=", 60))
	log.Printf("APPLICATION COMPLETED SUCCESSFULLY!")
	log.Printf(strings.Repeat("=", 60))
	log.Printf("Resources used:")
	log.Printf("  - S3 Bucket:          %s", result.BucketName)
	log.Printf("  - SQS Queue URL:      %s", result.QueueURL)
	log.Printf("  - CloudWatch Metrics: FileProcessing namespace")
	log.Printf("Summary:")
	log.Printf("  - Files processed:    %d", result.ProcessedFiles)
	log.Printf("  - Total bytes:        %d", result.TotalSize)
	log.Printf(strings.Repeat("=", 60))
	log.Printf("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")
}
