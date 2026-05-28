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
	"github.com/aws/aws-sdk-go-v2/service/lambda"
	lambdatypes "github.com/aws/aws-sdk-go-v2/service/lambda/types"
	"github.com/aws/aws-sdk-go-v2/service/sts"
)

// ── Config ────────────────────────────────────────────────────────────────────

// RunConfig mirrors the shape of config.json written by deploy.sh.
type RunConfig struct {
	FunctionName string `json:"functionName"`
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

// ── Main logic ────────────────────────────────────────────────────────────────

func run(ctx context.Context, cfg *RunConfig) error {
	log.Printf("Starting AWS Deployment Monitoring...")
	log.Printf("Using function:  %s", cfg.FunctionName)
	log.Printf("Using log group: %s", cfg.LogGroupName)
	log.Printf("Using region:    %s", cfg.Region)

	awsCfg, err := config.LoadDefaultConfig(ctx, config.WithRegion(cfg.Region))
	if err != nil {
		return fmt.Errorf("failed to load AWS config: %w", err)
	}

	stsClient    := sts.NewFromConfig(awsCfg)
	lambdaClient := lambda.NewFromConfig(awsCfg)

	// STS GetCallerIdentity
	identity, err := stsClient.GetCallerIdentity(ctx, &sts.GetCallerIdentityInput{})
	if err != nil {
		return fmt.Errorf("failed to get caller identity: %w", err)
	}
	log.Printf("Running as: %s", aws.ToString(identity.Arn))

	// Lambda InvokeFunction
	log.Printf("Invoking Lambda function: %s", cfg.FunctionName)
	response, err := lambdaClient.Invoke(ctx, &lambda.InvokeInput{
		FunctionName:   aws.String(cfg.FunctionName),
		InvocationType: lambdatypes.InvocationTypeRequestResponse,
	})
	if err != nil {
		return fmt.Errorf("failed to invoke Lambda function: %w", err)
	}

	log.Printf("Lambda invocation status: %d", response.StatusCode)
	log.Printf("Lambda response payload: %s", string(response.Payload))

	if response.StatusCode != 200 {
		return fmt.Errorf("Lambda invocation returned unexpected status: %d", response.StatusCode)
	}

	log.Printf(strings.Repeat("=", 60))
	log.Printf("APPLICATION COMPLETED SUCCESSFULLY!")
	log.Printf(strings.Repeat("=", 60))
	log.Printf("Resources used:")
	log.Printf("  - Lambda Function: %s", cfg.FunctionName)
	log.Printf("  - Log Group:       %s", cfg.LogGroupName)
	log.Printf(strings.Repeat("=", 60))
	log.Printf("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")

	return nil
}

// ── Entry point ───────────────────────────────────────────────────────────────

func main() {
	log.SetFlags(log.LstdFlags)
	log.SetPrefix("[DeploymentMonitor] ")

	ctx := context.Background()

	cfg, err := loadConfig()
	if err != nil {
		log.Fatalf("Configuration error: %v", err)
	}

	if err := run(ctx, cfg); err != nil {
		log.Fatalf("Application failed: %v", err)
	}
}
