package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/cloudwatchlogs"
	cwlTypes "github.com/aws/aws-sdk-go-v2/service/cloudwatchlogs/types"
	"github.com/aws/aws-sdk-go-v2/service/dynamodb"
	dbTypes "github.com/aws/aws-sdk-go-v2/service/dynamodb/types"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	s3Types "github.com/aws/aws-sdk-go-v2/service/s3/types"
	"github.com/aws/aws-sdk-go-v2/service/sts"
)

// ── Config ────────────────────────────────────────────────────────────────────

// RunConfig mirrors the shape of config.json written by deploy.sh.
type RunConfig struct {
	BucketName   string `json:"bucketName"`
	TableName    string `json:"tableName"`
	KmsKeyID     string `json:"kmsKeyId"`
	KmsKeyArn    string `json:"kmsKeyArn"`
	KmsAlias     string `json:"kmsAlias"`
	LogGroupName string `json:"logGroupName"`
	Region       string `json:"region"`
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

// ── Logging ───────────────────────────────────────────────────────────────────

func setupLogging() {
	log.SetFlags(log.LstdFlags)
	log.SetPrefix("[SecureDocMgr] ")
}

// ── Helpers ───────────────────────────────────────────────────────────────────

func sha256Hex(data []byte) string {
	h := sha256.Sum256(data)
	return hex.EncodeToString(h[:])
}

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

// ── Upload ────────────────────────────────────────────────────────────────────

type UploadResult struct {
	DocumentID string
	S3Key      string
	FileHash   string
}

func uploadDocument(
	ctx context.Context,
	s3Client *s3.Client,
	dbClient *dynamodb.Client,
	bucketName, tableName, kmsKeyID string,
	fileContent []byte,
	documentName string,
) (*UploadResult, error) {
	fileHash := sha256Hex(fileContent)
	rawID := sha256Hex([]byte(fmt.Sprintf("%s_%d", documentName, time.Now().UnixNano())))
	documentID := rawID[:16]
	s3Key := fmt.Sprintf("documents/%s/%s", documentID, documentName)

	// S3 PutObject with KMS encryption
	_, err := s3Client.PutObject(ctx, &s3.PutObjectInput{
		Bucket:               aws.String(bucketName),
		Key:                  aws.String(s3Key),
		Body:                 bytes.NewReader(fileContent),
		ServerSideEncryption: s3Types.ServerSideEncryptionAwsKms,
		SSEKMSKeyId:          aws.String(kmsKeyID),
		Metadata: map[string]string{
			"document-id":   documentID,
			"original-name": documentName,
		},
	})
	if err != nil {
		return nil, fmt.Errorf("failed to upload document to S3: %w", err)
	}
	log.Printf("Uploaded to s3://%s/%s", bucketName, s3Key)

	// DynamoDB PutItem — store metadata
	now := time.Now().UTC().Format(time.RFC3339)
	_, err = dbClient.PutItem(ctx, &dynamodb.PutItemInput{
		TableName: aws.String(tableName),
		Item: map[string]dbTypes.AttributeValue{
			"document_id":       &dbTypes.AttributeValueMemberS{Value: documentID},
			"document_name":     &dbTypes.AttributeValueMemberS{Value: documentName},
			"s3_bucket":         &dbTypes.AttributeValueMemberS{Value: bucketName},
			"s3_key":            &dbTypes.AttributeValueMemberS{Value: s3Key},
			"file_hash":         &dbTypes.AttributeValueMemberS{Value: fileHash},
			"file_size":         &dbTypes.AttributeValueMemberN{Value: fmt.Sprintf("%d", len(fileContent))},
			"upload_timestamp":  &dbTypes.AttributeValueMemberS{Value: now},
			"status":            &dbTypes.AttributeValueMemberS{Value: "active"},
		},
	})
	if err != nil {
		return nil, fmt.Errorf("failed to store document metadata in DynamoDB: %w", err)
	}
	log.Printf("Stored metadata in DynamoDB for document_id=%s", documentID)

	return &UploadResult{DocumentID: documentID, S3Key: s3Key, FileHash: fileHash}, nil
}

// ── Log operation ─────────────────────────────────────────────────────────────

func logOperation(
	ctx context.Context,
	cwlClient *cloudwatchlogs.Client,
	logGroupName, operation, documentID, documentName, status string,
) error {
	logStreamName := fmt.Sprintf("document-operations-%s", time.Now().UTC().Format("2006-01-02"))

	// CreateLogStream (ignore ResourceAlreadyExistsException)
	_, err := cwlClient.CreateLogStream(ctx, &cloudwatchlogs.CreateLogStreamInput{
		LogGroupName:  aws.String(logGroupName),
		LogStreamName: aws.String(logStreamName),
	})
	if err != nil {
		// Check if it's already-exists — that's fine
		var alreadyExists *cwlTypes.ResourceAlreadyExistsException
		if !isErrorType(err, &alreadyExists) {
			return fmt.Errorf("failed to create log stream: %w", err)
		}
	}

	entry := map[string]string{
		"timestamp":     time.Now().UTC().Format(time.RFC3339),
		"operation":     operation,
		"document_id":   documentID,
		"document_name": documentName,
		"status":        status,
	}
	entryJSON, _ := json.Marshal(entry)

	_, err = cwlClient.PutLogEvents(ctx, &cloudwatchlogs.PutLogEventsInput{
		LogGroupName:  aws.String(logGroupName),
		LogStreamName: aws.String(logStreamName),
		LogEvents: []cwlTypes.InputLogEvent{
			{
				Timestamp: aws.Int64(time.Now().UnixMilli()),
				Message:   aws.String(string(entryJSON)),
			},
		},
	})
	if err != nil {
		return fmt.Errorf("failed to put log events: %w", err)
	}
	log.Printf("Logged %s operation to CloudWatch", operation)
	return nil
}

// isErrorType checks if err is of the given target type (simple helper).
func isErrorType(err error, target interface{}) bool {
	// Use string matching as a fallback — the SDK wraps errors
	_ = target
	return strings.Contains(err.Error(), "ResourceAlreadyExistsException")
}

// ── List documents ────────────────────────────────────────────────────────────

func listDocuments(ctx context.Context, dbClient *dynamodb.Client, tableName string) ([]map[string]dbTypes.AttributeValue, error) {
	result, err := dbClient.Scan(ctx, &dynamodb.ScanInput{
		TableName: aws.String(tableName),
	})
	if err != nil {
		return nil, fmt.Errorf("failed to scan DynamoDB table: %w", err)
	}
	log.Printf("Found %d document(s) in DynamoDB", len(result.Items))
	return result.Items, nil
}

// ── Download document ─────────────────────────────────────────────────────────

func downloadDocument(
	ctx context.Context,
	s3Client *s3.Client,
	dbClient *dynamodb.Client,
	bucketName, tableName, documentID, downloadPath string,
) (string, error) {
	// DynamoDB GetItem — fetch metadata
	getResult, err := dbClient.GetItem(ctx, &dynamodb.GetItemInput{
		TableName: aws.String(tableName),
		Key: map[string]dbTypes.AttributeValue{
			"document_id": &dbTypes.AttributeValueMemberS{Value: documentID},
		},
	})
	if err != nil {
		return "", fmt.Errorf("failed to get document metadata: %w", err)
	}
	if getResult.Item == nil {
		return "", fmt.Errorf("document not found: %s", documentID)
	}

	s3Key := getResult.Item["s3_key"].(*dbTypes.AttributeValueMemberS).Value
	storedHash := getResult.Item["file_hash"].(*dbTypes.AttributeValueMemberS).Value
	docName := getResult.Item["document_name"].(*dbTypes.AttributeValueMemberS).Value

	// S3 GetObject
	getObjResult, err := s3Client.GetObject(ctx, &s3.GetObjectInput{
		Bucket: aws.String(bucketName),
		Key:    aws.String(s3Key),
	})
	if err != nil {
		return "", fmt.Errorf("failed to download document from S3: %w", err)
	}
	defer getObjResult.Body.Close()

	fileContent, err := io.ReadAll(getObjResult.Body)
	if err != nil {
		return "", fmt.Errorf("failed to read S3 object body: %w", err)
	}

	// Integrity check
	fileHash := sha256Hex(fileContent)
	if fileHash != storedHash {
		return "", fmt.Errorf("file integrity check failed")
	}

	if err := os.WriteFile(downloadPath, fileContent, 0644); err != nil {
		return "", fmt.Errorf("failed to write downloaded file: %w", err)
	}
	log.Printf("Downloaded document to %s", downloadPath)
	return docName, nil
}

// ── Main logic ────────────────────────────────────────────────────────────────

type DemoResult struct {
	AccountID      string
	DocumentID     string
	DocumentsCount int
}

func runDemo(ctx context.Context, cfg *RunConfig) (*DemoResult, error) {
	log.Println("Starting Secure Document Management System...")
	log.Printf("Using bucket:    %s", cfg.BucketName)
	log.Printf("Using table:     %s", cfg.TableName)
	log.Printf("Using KMS key:   %s", cfg.KmsKeyID)
	log.Printf("Using log group: %s", cfg.LogGroupName)
	log.Printf("Using region:    %s", cfg.Region)

	awsCfg, err := config.LoadDefaultConfig(ctx, config.WithRegion(cfg.Region))
	if err != nil {
		return nil, fmt.Errorf("failed to load AWS config: %w", err)
	}

	stsClient := sts.NewFromConfig(awsCfg)
	s3Client  := s3.NewFromConfig(awsCfg)
	dbClient  := dynamodb.NewFromConfig(awsCfg)
	cwlClient := cloudwatchlogs.NewFromConfig(awsCfg)

	// 1. STS GetCallerIdentity
	accountID, err := getAWSAccountID(ctx, stsClient)
	if err != nil {
		return nil, err
	}

	// 2. Create sample document
	sampleContent := []byte("This is a sample document for testing the secure document management system.")
	samplePath := "/tmp/sample_document.txt"
	if err := os.WriteFile(samplePath, sampleContent, 0644); err != nil {
		return nil, fmt.Errorf("failed to create sample document: %w", err)
	}
	log.Printf("Created sample document at %s", samplePath)

	// 3. S3 PutObject + DynamoDB PutItem
	log.Println("Uploading document...")
	uploadResult, err := uploadDocument(ctx, s3Client, dbClient,
		cfg.BucketName, cfg.TableName, cfg.KmsKeyID,
		sampleContent, "sample_document.txt")
	if err != nil {
		return nil, err
	}

	// 4. CloudWatch Logs — log UPLOAD
	if err := logOperation(ctx, cwlClient, cfg.LogGroupName,
		"UPLOAD", uploadResult.DocumentID, "sample_document.txt", "SUCCESS"); err != nil {
		return nil, fmt.Errorf("failed to log UPLOAD operation: %w", err)
	}

	// 5. DynamoDB Scan — list all documents
	docs, err := listDocuments(ctx, dbClient, cfg.TableName)
	if err != nil {
		return nil, err
	}

	// 6. S3 GetObject + DynamoDB GetItem — download
	downloadPath := "/tmp/downloaded_sample.txt"
	log.Println("Downloading document...")
	docName, err := downloadDocument(ctx, s3Client, dbClient,
		cfg.BucketName, cfg.TableName, uploadResult.DocumentID, downloadPath)
	if err != nil {
		return nil, err
	}

	// 7. CloudWatch Logs — log DOWNLOAD
	if err := logOperation(ctx, cwlClient, cfg.LogGroupName,
		"DOWNLOAD", uploadResult.DocumentID, docName, "SUCCESS"); err != nil {
		return nil, fmt.Errorf("failed to log DOWNLOAD operation: %w", err)
	}

	return &DemoResult{
		AccountID:      accountID,
		DocumentID:     uploadResult.DocumentID,
		DocumentsCount: len(docs),
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

	result, err := runDemo(ctx, cfg)
	if err != nil {
		log.Fatalf("Application failed: %v", err)
	}

	log.Printf(strings.Repeat("=", 60))
	log.Printf("APPLICATION COMPLETED SUCCESSFULLY!")
	log.Printf(strings.Repeat("=", 60))
	log.Printf("Resources used:")
	log.Printf("  - S3 Bucket:   %s", cfg.BucketName)
	log.Printf("  - DynamoDB:    %s", cfg.TableName)
	log.Printf("  - Log Group:   %s", cfg.LogGroupName)
	log.Printf("Summary:")
	log.Printf("  - Document ID:    %s", result.DocumentID)
	log.Printf("  - Total docs:     %d", result.DocumentsCount)
	log.Printf(strings.Repeat("=", 60))
	log.Printf("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")
}
