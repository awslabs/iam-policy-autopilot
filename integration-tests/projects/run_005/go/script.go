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

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/secretsmanager"
	"github.com/aws/aws-sdk-go-v2/service/sns"
	"github.com/aws/aws-sdk-go-v2/service/sts"
)

// ── Config ────────────────────────────────────────────────────────────────────

// RunConfig mirrors the shape of config.json written by deploy.sh.
type RunConfig struct {
	TopicArn   string `json:"topicArn"`
	SecretName string `json:"secretName"`
	SecretArn  string `json:"secretArn"`
	KmsKeyID   string `json:"kmsKeyId"`
	KmsKeyArn  string `json:"kmsKeyArn"`
	RepoName   string `json:"repoName"`
	CloneUrl   string `json:"cloneUrl"`
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

type MonitoringResult struct {
	AccountID  string `json:"account_id"`
	TopicArn   string `json:"topic_arn"`
	SecretName string `json:"secret_name"`
	RepoName   string `json:"repo_name"`
	CloneUrl   string `json:"clone_url"`
	Region     string `json:"region"`
}

// ── Logging ───────────────────────────────────────────────────────────────────

func setupLogging() {
	log.SetFlags(log.LstdFlags)
	log.SetPrefix("[SecureRepoMonitoring] ")
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

func retrieveSecret(ctx context.Context, smClient *secretsmanager.Client, secretName string) (map[string]interface{}, error) {
	log.Printf("Retrieving configuration from Secrets Manager: %s", secretName)
	result, err := smClient.GetSecretValue(ctx, &secretsmanager.GetSecretValueInput{
		SecretId: aws.String(secretName),
	})
	if err != nil {
		return nil, fmt.Errorf("failed to retrieve secret %s: %w", secretName, err)
	}

	var secretData map[string]interface{}
	if err := json.Unmarshal([]byte(*result.SecretString), &secretData); err != nil {
		return nil, fmt.Errorf("failed to parse secret JSON: %w", err)
	}
	log.Println("Successfully retrieved and decrypted configuration from Secrets Manager")
	return secretData, nil
}

func sendNotification(ctx context.Context, snsClient *sns.Client, topicArn, repoName, cloneUrl string) error {
	log.Printf("Sending repository notification via SNS to topic: %s", topicArn)

	message := map[string]string{
		"default": fmt.Sprintf("Secure repository '%s' is configured and ready.", repoName),
		"email": fmt.Sprintf(
			"Repository Monitoring Alert\n\nRepository: %s\nClone URL: %s\n\nSecurity features: KMS encryption, SNS notifications, Secrets Manager integration.",
			repoName, cloneUrl,
		),
	}
	messageJSON, err := json.Marshal(message)
	if err != nil {
		return fmt.Errorf("failed to marshal SNS message: %w", err)
	}

	subject := fmt.Sprintf("Repository Ready: %s", repoName)
	_, err = snsClient.Publish(ctx, &sns.PublishInput{
		TopicArn:         aws.String(topicArn),
		Message:          aws.String(string(messageJSON)),
		MessageStructure: aws.String("json"),
		Subject:          aws.String(subject),
	})
	if err != nil {
		return fmt.Errorf("failed to publish SNS notification: %w", err)
	}
	log.Println("Notification sent successfully")
	return nil
}

// ── Main logic ────────────────────────────────────────────────────────────────

func runSecureRepoMonitoring(ctx context.Context, cfg *RunConfig) (*MonitoringResult, error) {
	log.Println("Starting Secure Repository Monitoring...")
	log.Printf("Using SNS topic:  %s", cfg.TopicArn)
	log.Printf("Using secret:     %s", cfg.SecretName)
	log.Printf("Using repo:       %s", cfg.RepoName)
	log.Printf("Using region:     %s", cfg.Region)

	awsCfg, err := config.LoadDefaultConfig(ctx, config.WithRegion(cfg.Region))
	if err != nil {
		return nil, fmt.Errorf("failed to load AWS config: %w", err)
	}

	stsClient := sts.NewFromConfig(awsCfg)
	smClient  := secretsmanager.NewFromConfig(awsCfg)
	snsClient := sns.NewFromConfig(awsCfg)

	// 1. Get account ID
	accountID, err := getAWSAccountID(ctx, stsClient)
	if err != nil {
		return nil, err
	}

	// 2. Retrieve and verify secret from Secrets Manager
	log.Println("Retrieving configuration from Secrets Manager...")
	secretData, err := retrieveSecret(ctx, smClient, cfg.SecretName)
	if err != nil {
		return nil, err
	}
	if repoName, ok := secretData["repository_name"].(string); ok {
		log.Printf("Verified configuration for repository: %s", repoName)
	} else {
		log.Printf("Verified configuration for repository: %s", cfg.RepoName)
	}

	// 3. Send notification via SNS
	log.Println("Sending repository notification via SNS...")
	if err := sendNotification(ctx, snsClient, cfg.TopicArn, cfg.RepoName, cfg.CloneUrl); err != nil {
		return nil, err
	}

	return &MonitoringResult{
		AccountID:  accountID,
		TopicArn:   cfg.TopicArn,
		SecretName: cfg.SecretName,
		RepoName:   cfg.RepoName,
		CloneUrl:   cfg.CloneUrl,
		Region:     cfg.Region,
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

	result, err := runSecureRepoMonitoring(ctx, cfg)
	if err != nil {
		log.Fatalf("Application failed: %v", err)
	}

	log.Printf(strings.Repeat("=", 60))
	log.Printf("APPLICATION COMPLETED SUCCESSFULLY!")
	log.Printf(strings.Repeat("=", 60))
	log.Printf("Resources used:")
	log.Printf("  - SNS Topic:  %s", result.TopicArn)
	log.Printf("  - Secret:     %s", result.SecretName)
	log.Printf("  - Repo:       %s", result.RepoName)
	log.Printf("  - Clone URL:  %s", result.CloneUrl)
	log.Printf(strings.Repeat("=", 60))
	log.Printf("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")
}
