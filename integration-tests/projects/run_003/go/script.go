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
	s3Types "github.com/aws/aws-sdk-go-v2/service/s3/types"
	"github.com/aws/aws-sdk-go-v2/service/sfn"
	"github.com/aws/aws-sdk-go-v2/service/sts"
)

// ── Config ────────────────────────────────────────────────────────────────────

// RunConfig mirrors the shape of config.json written by deploy.sh.
type RunConfig struct {
	BucketName      string `json:"bucketName"`
	KmsKeyID        string `json:"kmsKeyId"`
	KmsKeyArn       string `json:"kmsKeyArn"`
	StateMachineArn string `json:"stateMachineArn"`
	LogGroupName    string `json:"logGroupName"`
	Region          string `json:"region"`
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

type SampleData struct {
	Timestamp int64  `json:"timestamp"`
	Data      string `json:"data"`
	Processed bool   `json:"processed"`
}

type ExecutionInput struct {
	Bucket    string `json:"bucket"`
	Timestamp int64  `json:"timestamp"`
}

type PipelineResult struct {
	AccountID       string `json:"account_id"`
	BucketName      string `json:"bucket_name"`
	DataKey         string `json:"data_key"`
	ExecutionArn    string `json:"execution_arn"`
	ExecutionStatus string `json:"execution_status"`
	StateMachineArn string `json:"state_machine_arn"`
	Region          string `json:"region"`
}

// ── Logging ───────────────────────────────────────────────────────────────────

func setupLogging() {
	log.SetFlags(log.LstdFlags)
	log.SetPrefix("[DataPipeline] ")
}

// ── Data-plane helpers ────────────────────────────────────────────────────────

func getAWSAccountID(ctx context.Context, stsClient *sts.Client) (string, error) {
	log.Println("Getting AWS account information...")
	result, err := stsClient.GetCallerIdentity(ctx, &sts.GetCallerIdentityInput{})
	if err != nil {
		return "", fmt.Errorf("failed to get AWS account ID: %w", err)
	}
	accountID := *result.Account
	log.Printf("AWS Account ID: %s", accountID)
	return accountID, nil
}

func uploadSampleData(ctx context.Context, s3Client *s3.Client, bucketName, kmsKeyID string, timestamp int64) (string, error) {
	key := fmt.Sprintf("data/sample-%d.json", timestamp)
	sampleData := SampleData{
		Timestamp: timestamp,
		Data:      "Sample data for processing pipeline",
		Processed: false,
	}
	body, err := json.Marshal(sampleData)
	if err != nil {
		return "", fmt.Errorf("failed to marshal sample data: %w", err)
	}

	_, err = s3Client.PutObject(ctx, &s3.PutObjectInput{
		Bucket:                  aws.String(bucketName),
		Key:                     aws.String(key),
		Body:                    strings.NewReader(string(body)),
		ContentType:             aws.String("application/json"),
		ServerSideEncryption:    s3Types.ServerSideEncryptionAwsKms,
		SSEKMSKeyId:             aws.String(kmsKeyID),
	})
	if err != nil {
		return "", fmt.Errorf("failed to upload sample data to S3: %w", err)
	}
	log.Printf("Uploaded sample data to s3://%s/%s", bucketName, key)
	return key, nil
}

func startPipelineExecution(ctx context.Context, sfnClient *sfn.Client, stateMachineArn, bucketName string, timestamp int64) (string, error) {
	inputData := ExecutionInput{Bucket: bucketName, Timestamp: timestamp}
	inputJSON, err := json.Marshal(inputData)
	if err != nil {
		return "", fmt.Errorf("failed to marshal execution input: %w", err)
	}

	result, err := sfnClient.StartExecution(ctx, &sfn.StartExecutionInput{
		StateMachineArn: aws.String(stateMachineArn),
		Input:           aws.String(string(inputJSON)),
	})
	if err != nil {
		return "", fmt.Errorf("failed to start Step Functions execution: %w", err)
	}
	executionArn := *result.ExecutionArn
	log.Printf("Started execution: %s", executionArn)
	return executionArn, nil
}

func pollExecution(ctx context.Context, sfnClient *sfn.Client, executionArn string, timeoutSeconds int) (string, error) {
	terminalStatuses := map[string]bool{
		"SUCCEEDED": true,
		"FAILED":    true,
		"TIMED_OUT": true,
		"ABORTED":   true,
	}
	deadline := time.Now().Add(time.Duration(timeoutSeconds) * time.Second)
	for time.Now().Before(deadline) {
		result, err := sfnClient.DescribeExecution(ctx, &sfn.DescribeExecutionInput{
			ExecutionArn: aws.String(executionArn),
		})
		if err != nil {
			return "", fmt.Errorf("failed to describe execution: %w", err)
		}
		status := string(result.Status)
		log.Printf("Execution status: %s", status)
		if terminalStatuses[status] {
			return status, nil
		}
		time.Sleep(5 * time.Second)
	}
	return "", fmt.Errorf("execution did not reach terminal state within %ds", timeoutSeconds)
}

func putPipelineMetrics(ctx context.Context, cwClient *cloudwatch.Client) error {
	namespace := "DataProcessingPipeline"
	now := time.Now().UTC()
	_, err := cwClient.PutMetricData(ctx, &cloudwatch.PutMetricDataInput{
		Namespace: aws.String(namespace),
		MetricData: []cwTypes.MetricDatum{
			{
				MetricName: aws.String("PipelineExecutions"),
				Value:      aws.Float64(1),
				Unit:       cwTypes.StandardUnitCount,
				Timestamp:  aws.Time(now),
			},
			{
				MetricName: aws.String("FilesProcessed"),
				Value:      aws.Float64(1),
				Unit:       cwTypes.StandardUnitCount,
				Timestamp:  aws.Time(now),
			},
		},
	})
	if err != nil {
		return fmt.Errorf("failed to put CloudWatch metrics: %w", err)
	}
	log.Printf("Published metrics to CloudWatch namespace '%s'", namespace)
	return nil
}

// ── Main logic ────────────────────────────────────────────────────────────────

func runDataPipeline(ctx context.Context, cfg *RunConfig) (*PipelineResult, error) {
	log.Println("Starting AWS Data Processing Pipeline...")
	log.Printf("Using bucket:        %s", cfg.BucketName)
	log.Printf("Using KMS key:       %s", cfg.KmsKeyID)
	log.Printf("Using state machine: %s", cfg.StateMachineArn)
	log.Printf("Using region:        %s", cfg.Region)

	awsCfg, err := config.LoadDefaultConfig(ctx, config.WithRegion(cfg.Region))
	if err != nil {
		return nil, fmt.Errorf("failed to load AWS config: %w", err)
	}

	s3Client  := s3.NewFromConfig(awsCfg)
	sfnClient := sfn.NewFromConfig(awsCfg)
	cwClient  := cloudwatch.NewFromConfig(awsCfg)
	stsClient := sts.NewFromConfig(awsCfg)

	// 1. Get account ID
	accountID, err := getAWSAccountID(ctx, stsClient)
	if err != nil {
		return nil, err
	}

	// 2. Upload sample data with KMS encryption
	timestamp := time.Now().Unix()
	log.Println("Uploading sample data to S3 with KMS encryption...")
	dataKey, err := uploadSampleData(ctx, s3Client, cfg.BucketName, cfg.KmsKeyID, timestamp)
	if err != nil {
		return nil, err
	}

	// 3. Start Step Functions execution
	log.Println("Starting Step Functions pipeline execution...")
	executionArn, err := startPipelineExecution(ctx, sfnClient, cfg.StateMachineArn, cfg.BucketName, timestamp)
	if err != nil {
		return nil, err
	}

	// 4. Poll for completion (60s timeout, 5s interval)
	log.Println("Polling for execution completion (timeout: 60s)...")
	finalStatus, err := pollExecution(ctx, sfnClient, executionArn, 60)
	if err != nil {
		return nil, err
	}
	log.Printf("Execution finished with status: %s", finalStatus)

	// 5. Put custom CloudWatch metrics
	log.Println("Publishing custom CloudWatch metrics...")
	if err := putPipelineMetrics(ctx, cwClient); err != nil {
		return nil, fmt.Errorf("failed to put CloudWatch metrics: %w", err)
	}

	return &PipelineResult{
		AccountID:       accountID,
		BucketName:      cfg.BucketName,
		DataKey:         dataKey,
		ExecutionArn:    executionArn,
		ExecutionStatus: finalStatus,
		StateMachineArn: cfg.StateMachineArn,
		Region:          cfg.Region,
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

	result, err := runDataPipeline(ctx, cfg)
	if err != nil {
		log.Fatalf("Application failed: %v", err)
	}

	log.Printf(strings.Repeat("=", 60))
	log.Printf("APPLICATION COMPLETED SUCCESSFULLY!")
	log.Printf(strings.Repeat("=", 60))
	log.Printf("Resources used:")
	log.Printf("  - S3 Bucket:          %s", result.BucketName)
	log.Printf("  - Data key:           %s", result.DataKey)
	log.Printf("  - State Machine:      %s", result.StateMachineArn)
	log.Printf("  - CloudWatch Metrics: DataProcessingPipeline namespace")
	log.Printf("Summary:")
	log.Printf("  - Execution ARN:      %s", result.ExecutionArn)
	log.Printf("  - Execution status:   %s", result.ExecutionStatus)
	log.Printf(strings.Repeat("=", 60))
	log.Printf("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")
}
