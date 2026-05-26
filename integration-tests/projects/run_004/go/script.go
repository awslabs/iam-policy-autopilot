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

	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/sts"
	"github.com/aws/aws-sdk-go-v2/service/xray"
	xrayTypes "github.com/aws/aws-sdk-go-v2/service/xray/types"
)

// ── Config ────────────────────────────────────────────────────────────────────

// RunConfig mirrors the shape of config.json written by deploy.sh.
type RunConfig struct {
	ClusterName       string `json:"clusterName"`
	ClusterArn        string `json:"clusterArn"`
	LogGroupName      string `json:"logGroupName"`
	KmsKeyID          string `json:"kmsKeyId"`
	KmsKeyArn         string `json:"kmsKeyArn"`
	ResourceGroupName string `json:"resourceGroupName"`
	Region            string `json:"region"`
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
	log.SetPrefix("[MLMonitoring] ")
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

func configureXRayEncryption(ctx context.Context, xrayClient *xray.Client) error {
	log.Println("Configuring X-Ray encryption (Type=NONE)...")
	_, err := xrayClient.PutEncryptionConfig(ctx, &xray.PutEncryptionConfigInput{
		Type: xrayTypes.EncryptionTypeNone,
	})
	if err != nil {
		return fmt.Errorf("failed to configure X-Ray encryption: %w", err)
	}
	log.Println("X-Ray encryption configured (Type=NONE)")
	return nil
}

// ── Main logic ────────────────────────────────────────────────────────────────

func runMLMonitoring(ctx context.Context, cfg *RunConfig) (string, error) {
	log.Println("Starting ML Monitoring Platform...")
	log.Printf("Using ECS cluster:    %s", cfg.ClusterName)
	log.Printf("Using log group:      %s", cfg.LogGroupName)
	log.Printf("Using KMS key:        %s", cfg.KmsKeyID)
	log.Printf("Using resource group: %s", cfg.ResourceGroupName)
	log.Printf("Using region:         %s", cfg.Region)

	awsCfg, err := config.LoadDefaultConfig(ctx, config.WithRegion(cfg.Region))
	if err != nil {
		return "", fmt.Errorf("failed to load AWS config: %w", err)
	}

	stsClient  := sts.NewFromConfig(awsCfg)
	xrayClient := xray.NewFromConfig(awsCfg)

	// 1. Get account ID
	accountID, err := getAWSAccountID(ctx, stsClient)
	if err != nil {
		return "", err
	}

	// 2. Configure X-Ray encryption (data-plane runtime config)
	if err := configureXRayEncryption(ctx, xrayClient); err != nil {
		return "", err
	}

	return accountID, nil
}

// ── Entry point ───────────────────────────────────────────────────────────────

func main() {
	setupLogging()
	ctx := context.Background()

	cfg, err := loadConfig()
	if err != nil {
		log.Fatalf("Configuration error: %v", err)
	}

	accountID, err := runMLMonitoring(ctx, cfg)
	if err != nil {
		log.Fatalf("Application failed: %v", err)
	}

	log.Printf(strings.Repeat("=", 60))
	log.Printf("APPLICATION COMPLETED SUCCESSFULLY!")
	log.Printf(strings.Repeat("=", 60))
	log.Printf("Resources used:")
	log.Printf("  - ECS Cluster:    %s", cfg.ClusterName)
	log.Printf("  - Log Group:      %s", cfg.LogGroupName)
	log.Printf("  - KMS Key:        %s", cfg.KmsKeyID)
	log.Printf("  - Resource Group: %s", cfg.ResourceGroupName)
	log.Printf("Summary:")
	log.Printf("  - AWS Account ID: %s", accountID)
	log.Printf("  - Region:         %s", cfg.Region)
	log.Printf(strings.Repeat("=", 60))
	log.Printf("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")
}
