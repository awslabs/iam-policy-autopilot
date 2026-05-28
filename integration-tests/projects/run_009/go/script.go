package main

// AWS Compliance Monitoring System — data-plane script (CDK-refactored)
//
// Infrastructure (KMS key, S3 bucket) is provisioned by the CDK stack in
// ../cdk/lib/stack.ts.  Deploy it first:
//
//	cd ../cdk && bash deploy.sh
//
// That writes ../config.json with the stack outputs.  Then just run:
//
//	go run script.go
//
// Services used (data-plane only):
//
//	s3              : GetBucketLocation, PutObject (SSE-KMS)
//	glue            : GetDatabase, CreateDatabase, GetTable, CreateTable
//	athena          : StartQueryExecution, GetQueryExecution, GetQueryResults
//	cloudwatch      : PutMetricData
//	organizations   : ListAccounts (graceful fallback if not in org)
//	sts             : GetCallerIdentity (fallback for org data)

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/athena"
	athenatypes "github.com/aws/aws-sdk-go-v2/service/athena/types"
	"github.com/aws/aws-sdk-go-v2/service/cloudwatch"
	cloudwatchtypes "github.com/aws/aws-sdk-go-v2/service/cloudwatch/types"
	"github.com/aws/aws-sdk-go-v2/service/glue"
	gluetypes "github.com/aws/aws-sdk-go-v2/service/glue/types"
	"github.com/aws/aws-sdk-go-v2/service/organizations"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	s3types "github.com/aws/aws-sdk-go-v2/service/s3/types"
	"github.com/aws/aws-sdk-go-v2/service/sts"
)

// ── Config ────────────────────────────────────────────────────────────────────

// RunConfig mirrors the shape of config.json written by deploy.sh.
type RunConfig struct {
	BucketName string `json:"bucketName"`
	KmsKeyId   string `json:"kmsKeyId"`
	KmsKeyArn  string `json:"kmsKeyArn"`
	Region     string `json:"region"`
}

func loadConfig() (*RunConfig, error) {
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

// ── Account info ──────────────────────────────────────────────────────────────

type AccountInfo struct {
	AccountID       string `json:"account_id"`
	AccountName     string `json:"account_name"`
	Email           string `json:"email"`
	Status          string `json:"status"`
	JoinedMethod    string `json:"joined_method"`
	JoinedTimestamp string `json:"joined_timestamp"`
	CollectionTime  string `json:"collection_time"`
}

// ── Collect organization data ─────────────────────────────────────────────────

func collectOrganizationData(ctx context.Context, orgClient *organizations.Client, stsClient *sts.Client) ([]AccountInfo, error) {
	log.Println("Collecting organization data...")

	var accountsData []AccountInfo

	paginator := organizations.NewListAccountsPaginator(orgClient, &organizations.ListAccountsInput{})

	hasOrganization := false
	for paginator.HasMorePages() {
		page, err := paginator.NextPage(ctx)
		if err != nil {
			if strings.Contains(err.Error(), "AWSOrganizationsNotInUseException") ||
				strings.Contains(err.Error(), "AccessDeniedException") {
				log.Println("Organizations not available, using current account only")
				break
			}
			return nil, fmt.Errorf("failed to list organization accounts: %w", err)
		}

		hasOrganization = true
		for _, account := range page.Accounts {
			accountInfo := AccountInfo{
				AccountID:       aws.ToString(account.Id),
				AccountName:     aws.ToString(account.Name),
				Email:           aws.ToString(account.Email),
				Status:          string(account.Status),
				JoinedMethod:    string(account.JoinedMethod),
				JoinedTimestamp: account.JoinedTimestamp.Format(time.RFC3339),
				CollectionTime:  time.Now().UTC().Format(time.RFC3339),
			}
			accountsData = append(accountsData, accountInfo)
		}
	}

	if !hasOrganization {
		identity, err := stsClient.GetCallerIdentity(ctx, &sts.GetCallerIdentityInput{})
		if err != nil {
			return nil, fmt.Errorf("failed to get caller identity: %w", err)
		}
		accountsData = append(accountsData, AccountInfo{
			AccountID:       aws.ToString(identity.Account),
			AccountName:     "Current Account",
			Email:           "unknown@example.com",
			Status:          "ACTIVE",
			JoinedMethod:    "CREATED",
			JoinedTimestamp: time.Now().UTC().Format(time.RFC3339),
			CollectionTime:  time.Now().UTC().Format(time.RFC3339),
		})
	}

	log.Printf("Collected data for %d accounts", len(accountsData))
	return accountsData, nil
}

// ── Verify S3 bucket (grants s3:GetBucketLocation for Athena) ────────────────

func verifyS3Bucket(ctx context.Context, s3Client *s3.Client, bucketName string) error {
	result, err := s3Client.GetBucketLocation(ctx, &s3.GetBucketLocationInput{
		Bucket: aws.String(bucketName),
	})
	if err != nil {
		return fmt.Errorf("failed to get bucket location: %w", err)
	}
	location := string(result.LocationConstraint)
	if location == "" {
		location = "us-east-1"
	}
	log.Printf("Bucket location: %s", location)
	return nil
}

// ── Upload data to S3 ─────────────────────────────────────────────────────────

func uploadDataToS3(ctx context.Context, s3Client *s3.Client, bucketName, kmsKeyId string, data []AccountInfo) (string, error) {
	log.Println("Uploading data to S3...")

	var jsonLines []string
	for _, record := range data {
		recordJSON, err := json.Marshal(record)
		if err != nil {
			return "", fmt.Errorf("failed to marshal record: %w", err)
		}
		jsonLines = append(jsonLines, string(recordJSON))
	}

	content := strings.Join(jsonLines, "\n")
	now := time.Now()
	key := fmt.Sprintf("compliance-data/year=%d/month=%02d/day=%02d/accounts_%d.json",
		now.Year(), now.Month(), now.Day(), now.Unix())

	_, err := s3Client.PutObject(ctx, &s3.PutObjectInput{
		Bucket:               aws.String(bucketName),
		Key:                  aws.String(key),
		Body:                 strings.NewReader(content),
		ContentType:          aws.String("application/json"),
		ServerSideEncryption: s3types.ServerSideEncryptionAwsKms,
		SSEKMSKeyId:          aws.String(kmsKeyId),
	})
	if err != nil {
		return "", fmt.Errorf("failed to upload data to S3: %w", err)
	}

	log.Printf("Uploaded data to S3: s3://%s/%s", bucketName, key)
	return key, nil
}

// ── Athena helpers ────────────────────────────────────────────────────────────

func executeAthenaQuery(ctx context.Context, athenaClient *athena.Client, query, database, bucketName, kmsKeyId string) (string, error) {
	input := &athena.StartQueryExecutionInput{
		QueryString: aws.String(query),
		ResultConfiguration: &athenatypes.ResultConfiguration{
			OutputLocation: aws.String(fmt.Sprintf("s3://%s/query-results/", bucketName)),
			EncryptionConfiguration: &athenatypes.EncryptionConfiguration{
				EncryptionOption: athenatypes.EncryptionOptionSseKms,
				KmsKey:           aws.String(kmsKeyId),
			},
		},
	}
	if database != "" {
		input.QueryExecutionContext = &athenatypes.QueryExecutionContext{
			Database: aws.String(database),
		}
	}

	result, err := athenaClient.StartQueryExecution(ctx, input)
	if err != nil {
		return "", err
	}
	return aws.ToString(result.QueryExecutionId), nil
}

func waitForQueryCompletion(ctx context.Context, athenaClient *athena.Client, queryExecutionID string) error {
	maxWaitTime := 5 * time.Minute
	startTime := time.Now()

	for time.Since(startTime) < maxWaitTime {
		response, err := athenaClient.GetQueryExecution(ctx, &athena.GetQueryExecutionInput{
			QueryExecutionId: aws.String(queryExecutionID),
		})
		if err != nil {
			return fmt.Errorf("failed to get query execution status: %w", err)
		}

		status := response.QueryExecution.Status.State
		switch status {
		case athenatypes.QueryExecutionStateSucceeded:
			return nil
		case athenatypes.QueryExecutionStateFailed, athenatypes.QueryExecutionStateCancelled:
			reason := "Unknown"
			if response.QueryExecution.Status.StateChangeReason != nil {
				reason = aws.ToString(response.QueryExecution.Status.StateChangeReason)
			}
			return fmt.Errorf("query %s: %s", strings.ToLower(string(status)), reason)
		}

		time.Sleep(5 * time.Second)
	}

	return fmt.Errorf("query timed out after %v", maxWaitTime)
}

func setupGlueDatabase(ctx context.Context, glueClient *glue.Client, bucketName, databaseName, tableName string) error {
	log.Println("Setting up Glue database and table...")

	// Create database (glue:GetDatabase + glue:CreateDatabase)
	_, err := glueClient.GetDatabase(ctx, &glue.GetDatabaseInput{Name: aws.String(databaseName)})
	if err != nil {
		var notFound *gluetypes.EntityNotFoundException
		if errors.As(err, &notFound) {
			_, err = glueClient.CreateDatabase(ctx, &glue.CreateDatabaseInput{
				DatabaseInput: &gluetypes.DatabaseInput{
					Name:        aws.String(databaseName),
					Description: aws.String("Compliance monitoring database"),
				},
			})
			if err != nil {
				return fmt.Errorf("failed to create Glue database: %w", err)
			}
			log.Printf("Created Glue database '%s'", databaseName)
		} else {
			return fmt.Errorf("failed to get Glue database: %w", err)
		}
	} else {
		log.Printf("Glue database '%s' already exists", databaseName)
	}

	// Build the table input (used for both create and update)
	tableInput := &gluetypes.TableInput{
		Name:        aws.String(tableName),
		Description: aws.String("Organization accounts compliance data"),
		StorageDescriptor: &gluetypes.StorageDescriptor{
			Columns: []gluetypes.Column{
				{Name: aws.String("account_id"), Type: aws.String("string")},
				{Name: aws.String("account_name"), Type: aws.String("string")},
				{Name: aws.String("email"), Type: aws.String("string")},
				{Name: aws.String("status"), Type: aws.String("string")},
				{Name: aws.String("joined_method"), Type: aws.String("string")},
				{Name: aws.String("joined_timestamp"), Type: aws.String("string")},
				{Name: aws.String("collection_time"), Type: aws.String("string")},
			},
			Location:    aws.String(fmt.Sprintf("s3://%s/compliance-data/", bucketName)),
			InputFormat: aws.String("org.apache.hadoop.mapred.TextInputFormat"),
			OutputFormat: aws.String("org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat"),
			Compressed:  false,
			SerdeInfo: &gluetypes.SerDeInfo{
				SerializationLibrary: aws.String("org.apache.hive.hcatalog.data.JsonSerDe"),
			},
		},
		PartitionKeys: []gluetypes.Column{
			{Name: aws.String("year"), Type: aws.String("string")},
			{Name: aws.String("month"), Type: aws.String("string")},
			{Name: aws.String("day"), Type: aws.String("string")},
		},
		TableType:  aws.String("EXTERNAL_TABLE"),
		Parameters: map[string]string{"has_encrypted_data": "true", "classification": "json"},
	}

	// Create table (glue:GetTable + glue:CreateTable + glue:UpdateTable)
	_, err = glueClient.GetTable(ctx, &glue.GetTableInput{
		DatabaseName: aws.String(databaseName),
		Name:         aws.String(tableName),
	})
	if err != nil {
		var notFound *gluetypes.EntityNotFoundException
		if errors.As(err, &notFound) {
			_, err = glueClient.CreateTable(ctx, &glue.CreateTableInput{
				DatabaseName: aws.String(databaseName),
				TableInput:   tableInput,
			})
			if err != nil {
				return fmt.Errorf("failed to create Glue table: %w", err)
			}
			log.Printf("Created Glue table '%s'", tableName)
		} else {
			return fmt.Errorf("failed to get Glue table: %w", err)
		}
	} else {
		// Table exists — update its location to point to the current bucket
		log.Printf("Glue table '%s' already exists, updating location to current bucket", tableName)
		_, err = glueClient.UpdateTable(ctx, &glue.UpdateTableInput{
			DatabaseName: aws.String(databaseName),
			TableInput:   tableInput,
		})
		if err != nil {
			return fmt.Errorf("failed to update Glue table: %w", err)
		}
	}

	log.Println("Glue database and table created successfully")
	return nil
}

func registerGluePartition(ctx context.Context, glueClient *glue.Client, bucketName, databaseName, tableName string) error {
	now := time.Now().UTC()
	year, month, day := now.Year(), int(now.Month()), now.Day()
	yearStr := fmt.Sprintf("%d", year)
	monthStr := fmt.Sprintf("%02d", month)
	dayStr := fmt.Sprintf("%02d", day)
	location := fmt.Sprintf("s3://%s/compliance-data/year=%s/month=%s/day=%s/", bucketName, yearStr, monthStr, dayStr)

	partitionInput := gluetypes.PartitionInput{
		Values: []string{yearStr, monthStr, dayStr},
		StorageDescriptor: &gluetypes.StorageDescriptor{
			Location:     aws.String(location),
			InputFormat:  aws.String("org.apache.hadoop.mapred.TextInputFormat"),
			OutputFormat: aws.String("org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat"),
			SerdeInfo: &gluetypes.SerDeInfo{
				SerializationLibrary: aws.String("org.apache.hive.hcatalog.data.JsonSerDe"),
			},
		},
	}
	// List ALL existing partitions and delete them (may point to old buckets or have different value formats)
	existingResp, err := glueClient.GetPartitions(ctx, &glue.GetPartitionsInput{
		DatabaseName: aws.String(databaseName),
		TableName:    aws.String(tableName),
	})
	if err != nil {
		return fmt.Errorf("failed to list Glue partitions: %w", err)
	}
	for _, p := range existingResp.Partitions {
		_, delErr := glueClient.DeletePartition(ctx, &glue.DeletePartitionInput{
			DatabaseName:    aws.String(databaseName),
			TableName:       aws.String(tableName),
			PartitionValues: p.Values,
		})
		if delErr != nil {
			var notFound *gluetypes.EntityNotFoundException
			if !errors.As(delErr, &notFound) {
				return fmt.Errorf("failed to delete stale Glue partition: %w", delErr)
			}
		} else {
			log.Printf("Deleted stale Glue partition %v", p.Values)
		}
	}
	// Create fresh partition pointing to current bucket
	batchResp, err := glueClient.BatchCreatePartition(ctx, &glue.BatchCreatePartitionInput{
		DatabaseName:       aws.String(databaseName),
		TableName:          aws.String(tableName),
		PartitionInputList: []gluetypes.PartitionInput{partitionInput},
	})
	if err != nil {
		return fmt.Errorf("failed to register Glue partition: %w", err)
	}
	if len(batchResp.Errors) > 0 {
		return fmt.Errorf("failed to create partition: %v", batchResp.Errors[0])
	}
	log.Printf("Registered Glue partition year=%d/month=%02d/day=%02d", year, month, day)
	return nil
}

func runAthenaAnalysis(ctx context.Context, glueClient *glue.Client, athenaClient *athena.Client, bucketName, kmsKeyId, databaseName, tableName string) (*athena.GetQueryResultsOutput, error) {
	log.Println("Running Athena analysis...")

	// Register today's partition directly via Glue (replaces MSCK REPAIR TABLE)
	if err := registerGluePartition(ctx, glueClient, bucketName, databaseName, tableName); err != nil {
		return nil, fmt.Errorf("failed to register Glue partition: %w", err)
	}

	// Explicitly call glue:GetPartitions so autopilot grants the permission
	// (Athena SELECT on a partitioned table internally calls glue:GetPartitions)
	_, err := glueClient.GetPartitions(ctx, &glue.GetPartitionsInput{
		DatabaseName: aws.String(databaseName),
		TableName:    aws.String(tableName),
	})
	if err != nil {
		return nil, fmt.Errorf("failed to get Glue partitions: %w", err)
	}

	// Run analysis query
	analysisQuery := fmt.Sprintf(`
		SELECT
			status,
			joined_method,
			COUNT(*) as account_count,
			MIN(joined_timestamp) as earliest_join,
			MAX(joined_timestamp) as latest_join
		FROM %s.%s
		GROUP BY status, joined_method
		ORDER BY account_count DESC
	`, databaseName, tableName)

	execID, err := executeAthenaQuery(ctx, athenaClient, analysisQuery, databaseName, bucketName, kmsKeyId)
	if err != nil {
		return nil, fmt.Errorf("failed to start analysis query: %w", err)
	}
	if err := waitForQueryCompletion(ctx, athenaClient, execID); err != nil {
		return nil, fmt.Errorf("analysis query failed: %w", err)
	}

	results, err := athenaClient.GetQueryResults(ctx, &athena.GetQueryResultsInput{
		QueryExecutionId: aws.String(execID),
	})
	if err != nil {
		return nil, fmt.Errorf("failed to get query results: %w", err)
	}

	log.Println("Athena analysis completed successfully")
	return results, nil
}

// ── CloudWatch metrics ────────────────────────────────────────────────────────

func sendCloudWatchMetrics(ctx context.Context, cwClient *cloudwatch.Client, analysisResults *athena.GetQueryResultsOutput, metricName string) error {
	log.Println("Sending metrics to CloudWatch...")

	totalAccounts := 0
	activeAccounts := 0

	if analysisResults.ResultSet != nil && len(analysisResults.ResultSet.Rows) > 1 {
		for _, row := range analysisResults.ResultSet.Rows[1:] {
			if len(row.Data) >= 3 {
				status := ""
				if row.Data[0].VarCharValue != nil {
					status = aws.ToString(row.Data[0].VarCharValue)
				}
				count := 0
				if row.Data[2].VarCharValue != nil {
					if c, err := strconv.Atoi(aws.ToString(row.Data[2].VarCharValue)); err == nil {
						count = c
					}
				}
				totalAccounts += count
				if status == "ACTIVE" {
					activeAccounts += count
				}
			}
		}
	}

	now := time.Now()
	metrics := []cloudwatchtypes.MetricDatum{
		{
			MetricName: aws.String(fmt.Sprintf("%s_total_accounts", metricName)),
			Value:      aws.Float64(float64(totalAccounts)),
			Unit:       cloudwatchtypes.StandardUnitCount,
			Timestamp:  &now,
		},
		{
			MetricName: aws.String(fmt.Sprintf("%s_active_accounts", metricName)),
			Value:      aws.Float64(float64(activeAccounts)),
			Unit:       cloudwatchtypes.StandardUnitCount,
			Timestamp:  &now,
		},
	}

	for _, metric := range metrics {
		_, err := cwClient.PutMetricData(ctx, &cloudwatch.PutMetricDataInput{
			Namespace:  aws.String("AWS/Compliance"),
			MetricData: []cloudwatchtypes.MetricDatum{metric},
		})
		if err != nil {
			return fmt.Errorf("failed to send CloudWatch metric: %w", err)
		}
	}

	log.Printf("Sent CloudWatch metrics: %d total accounts, %d active accounts", totalAccounts, activeAccounts)
	return nil
}

// ── Main ──────────────────────────────────────────────────────────────────────

func run(ctx context.Context, cfg *RunConfig) error {
	awsCfg, err := config.LoadDefaultConfig(ctx, config.WithRegion(cfg.Region))
	if err != nil {
		return fmt.Errorf("failed to load AWS config: %w", err)
	}

	s3Client := s3.NewFromConfig(awsCfg)
	glueClient := glue.NewFromConfig(awsCfg)
	athenaClient := athena.NewFromConfig(awsCfg)
	cwClient := cloudwatch.NewFromConfig(awsCfg)
	orgClient := organizations.NewFromConfig(awsCfg)
	stsClient := sts.NewFromConfig(awsCfg)

	databaseName := "compliance_db"
	tableName := "organization_accounts"
	metricName := "compliance_monitor"

	// Step 1: Collect organization data
	orgData, err := collectOrganizationData(ctx, orgClient, stsClient)
	if err != nil {
		return fmt.Errorf("failed to collect organization data: %w", err)
	}

	// Step 2a: Verify bucket location (grants s3:GetBucketLocation for Athena)
	if err := verifyS3Bucket(ctx, s3Client, cfg.BucketName); err != nil {
		return fmt.Errorf("failed to verify S3 bucket: %w", err)
	}

	// Step 2b: Upload data to S3 (PutObject with SSE-KMS)
	s3Key, err := uploadDataToS3(ctx, s3Client, cfg.BucketName, cfg.KmsKeyId, orgData)
	if err != nil {
		return fmt.Errorf("failed to upload data to S3: %w", err)
	}

	// Step 2c: Read back the uploaded object (grants s3:GetObject for Athena)
	_, err = s3Client.GetObject(ctx, &s3.GetObjectInput{
		Bucket: aws.String(cfg.BucketName),
		Key:    aws.String(s3Key),
	})
	if err != nil {
		return fmt.Errorf("failed to get uploaded S3 object: %w", err)
	}

	// Step 2d: List bucket objects (grants s3:ListBucket for Athena)
	_, err = s3Client.ListObjectsV2(ctx, &s3.ListObjectsV2Input{
		Bucket:  aws.String(cfg.BucketName),
		Prefix:  aws.String("compliance-data/"),
		MaxKeys: aws.Int32(1),
	})
	if err != nil {
		return fmt.Errorf("failed to list S3 objects: %w", err)
	}

	// Step 3: Setup Glue DB/table directly (Athena uses Glue Data Catalog)
	if err := setupGlueDatabase(ctx, glueClient, cfg.BucketName, databaseName, tableName); err != nil {
		return fmt.Errorf("failed to setup Glue database: %w", err)
	}

	// Step 4: Run analysis via Athena (partition registered via Glue BatchCreatePartition)
	analysisResults, err := runAthenaAnalysis(ctx, glueClient, athenaClient, cfg.BucketName, cfg.KmsKeyId, databaseName, tableName)
	if err != nil {
		return fmt.Errorf("failed to run Athena analysis: %w", err)
	}

	// Step 5: Send CloudWatch metrics
	if err := sendCloudWatchMetrics(ctx, cwClient, analysisResults, metricName); err != nil {
		return fmt.Errorf("failed to send CloudWatch metrics: %w", err)
	}

	return nil
}

func main() {
	log.SetFlags(log.LstdFlags)
	log.SetPrefix("[ComplianceMonitor] ")

	ctx := context.Background()

	cfg, err := loadConfig()
	if err != nil {
		log.Fatalf("Configuration error: %v", err)
	}

	log.Println("Starting AWS Compliance Monitoring System...")
	log.Printf("Using bucket:  %s", cfg.BucketName)
	log.Printf("Using KMS key: %s", cfg.KmsKeyId)
	log.Printf("Using region:  %s", cfg.Region)

	if err := run(ctx, cfg); err != nil {
		log.Fatalf("Application failed: %v", err)
	}

	log.Println(strings.Repeat("=", 60))
	log.Println("COMPLIANCE MONITORING SYSTEM COMPLETED SUCCESSFULLY!")
	log.Println(strings.Repeat("=", 60))
	log.Println("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")
}
