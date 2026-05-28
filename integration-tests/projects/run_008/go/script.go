package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/cloudwatchlogs"
	cwltypes "github.com/aws/aws-sdk-go-v2/service/cloudwatchlogs/types"
	"github.com/aws/aws-sdk-go-v2/service/servicecatalog"
	"github.com/aws/aws-sdk-go-v2/service/sts"
	smithy "github.com/aws/smithy-go"
)

// RunConfig holds the configuration loaded from config.json
type RunConfig struct {
	LogGroupName string `json:"logGroupName"`
	Region       string `json:"region"`
}

// PortfolioInfo represents portfolio information
type PortfolioInfo struct {
	ID       string        `json:"id"`
	Name     string        `json:"name"`
	Products []ProductInfo `json:"products"`
}

// ProductInfo represents product information
type ProductInfo struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

// loadConfig reads ../config.json relative to this source file's directory.
func loadConfig() (*RunConfig, error) {
	_, filename, _, ok := runtime.Caller(0)
	if !ok {
		return nil, fmt.Errorf("could not determine source file path")
	}
	configPath := filepath.Join(filepath.Dir(filename), "..", "config.json")
	configPath, err := filepath.Abs(configPath)
	if err != nil {
		return nil, fmt.Errorf("failed to resolve config path: %w", err)
	}

	data, err := os.ReadFile(configPath)
	if err != nil {
		return nil, fmt.Errorf(
			"config.json not found at %s.\nDeploy the CDK stack first:\n  cd ../cdk && bash deploy.sh",
			configPath,
		)
	}

	var cfg RunConfig
	if err := json.Unmarshal(data, &cfg); err != nil {
		return nil, fmt.Errorf("failed to parse config.json: %w", err)
	}
	return &cfg, nil
}

// logToCloudWatch creates a log stream (if needed) and puts a log event.
// Returns an error on permission failures (e.g. AccessDeniedException) so the
// minimizer can detect that the IAM actions are required.
func logToCloudWatch(ctx context.Context, logsClient *cloudwatchlogs.Client, logGroupName, logStreamName, message string) error {
	// Create log stream (ignore only ResourceAlreadyExistsException)
	_, err := logsClient.CreateLogStream(ctx, &cloudwatchlogs.CreateLogStreamInput{
		LogGroupName:  aws.String(logGroupName),
		LogStreamName: aws.String(logStreamName),
	})
	if err != nil {
		var apiErr smithy.APIError
		if errors.As(err, &apiErr) && apiErr.ErrorCode() == "ResourceAlreadyExistsException" {
			// Expected — stream already exists, continue
		} else {
			return fmt.Errorf("CreateLogStream failed: %w", err)
		}
	}

	logEvent := cwltypes.InputLogEvent{
		Timestamp: aws.Int64(time.Now().UnixMilli()),
		Message:   aws.String(message),
	}

	_, err = logsClient.PutLogEvents(ctx, &cloudwatchlogs.PutLogEventsInput{
		LogGroupName:  aws.String(logGroupName),
		LogStreamName: aws.String(logStreamName),
		LogEvents:     []cwltypes.InputLogEvent{logEvent},
	})
	if err != nil {
		return fmt.Errorf("PutLogEvents failed: %w", err)
	}

	log.Printf("Logged to CloudWatch: %s", message)
	return nil
}

// listPortfoliosAndProducts lists all portfolios and searches products within each.
func listPortfoliosAndProducts(
	ctx context.Context,
	scClient *servicecatalog.Client,
	logsClient *cloudwatchlogs.Client,
	logGroupName, logStreamName string,
) ([]PortfolioInfo, error) {
	portfoliosResp, err := scClient.ListPortfolios(ctx, &servicecatalog.ListPortfoliosInput{})
	if err != nil {
		return nil, fmt.Errorf("failed to list portfolios: %w", err)
	}

	var portfolioInfo []PortfolioInfo
	for _, portfolio := range portfoliosResp.PortfolioDetails {
		portfolioID := *portfolio.Id
		portfolioName := *portfolio.DisplayName

		var productList []ProductInfo
		productsResp, err := scClient.SearchProductsAsAdmin(ctx, &servicecatalog.SearchProductsAsAdminInput{
			PortfolioId: aws.String(portfolioID),
		})
		if err == nil {
			for _, product := range productsResp.ProductViewDetails {
				productList = append(productList, ProductInfo{
					ID:   *product.ProductViewSummary.ProductId,
					Name: *product.ProductViewSummary.Name,
				})
			}
		} else {
			log.Printf("Warning: Failed to get products for portfolio %s: %v", portfolioID, err)
		}

		portfolioInfo = append(portfolioInfo, PortfolioInfo{
			ID:       portfolioID,
			Name:     portfolioName,
			Products: productList,
		})
	}

	infoMsg := fmt.Sprintf("Found %d portfolios", len(portfolioInfo))
	log.Println(infoMsg)
	if err := logToCloudWatch(ctx, logsClient, logGroupName, logStreamName, infoMsg); err != nil {
		return nil, fmt.Errorf("failed to log info message: %w", err)
	}

	for _, p := range portfolioInfo {
		detailMsg := fmt.Sprintf("Portfolio: %s (%s) has %d products", p.Name, p.ID, len(p.Products))
		log.Println(detailMsg)
		if err := logToCloudWatch(ctx, logsClient, logGroupName, logStreamName, detailMsg); err != nil {
			return nil, fmt.Errorf("failed to log detail message: %w", err)
		}
	}

	return portfolioInfo, nil
}

func main() {
	log.SetOutput(os.Stderr)
	log.SetFlags(log.LstdFlags | log.Lmicroseconds)

	ctx := context.Background()

	// Load config
	cfg, err := loadConfig()
	if err != nil {
		log.Fatalf("Failed to load config: %v", err)
	}

	log.Printf("Starting AWS Service Catalog Manager")
	log.Printf("Log group: %s", cfg.LogGroupName)
	log.Printf("Region:    %s", cfg.Region)

	// Load AWS config
	awsCfg, err := config.LoadDefaultConfig(ctx, config.WithRegion(cfg.Region))
	if err != nil {
		log.Fatalf("Unable to load SDK config: %v", err)
	}

	logsClient := cloudwatchlogs.NewFromConfig(awsCfg)
	scClient := servicecatalog.NewFromConfig(awsCfg)
	stsClient := sts.NewFromConfig(awsCfg)

	// Verify credentials
	identity, err := stsClient.GetCallerIdentity(ctx, &sts.GetCallerIdentityInput{})
	if err != nil {
		log.Fatalf("Failed to get caller identity: %v", err)
	}
	log.Printf("AWS Account: %s", *identity.Account)

	logStreamName := fmt.Sprintf("service-catalog-manager-%d", time.Now().Unix())

	// Log startup
	if err := logToCloudWatch(ctx, logsClient, cfg.LogGroupName, logStreamName,
		"Service Catalog Manager started"); err != nil {
		log.Fatalf("Failed to log startup message: %v", err)
	}

	// List portfolios and products
	log.Printf("Listing portfolios and products...")
	portfolioInfo, err := listPortfoliosAndProducts(ctx, scClient, logsClient,
		cfg.LogGroupName, logStreamName)
	if err != nil {
		log.Fatalf("Failed to list portfolios: %v", err)
	}

	// Log completion
	completionMsg := "Service Catalog Manager completed successfully"
	if err := logToCloudWatch(ctx, logsClient, cfg.LogGroupName, logStreamName, completionMsg); err != nil {
		log.Fatalf("Failed to log completion message: %v", err)
	}

	log.Printf("============================================================")
	log.Printf("SERVICE CATALOG MANAGER COMPLETED")
	log.Printf("============================================================")
	log.Printf("Region:           %s", cfg.Region)
	log.Printf("Log Group:        %s", cfg.LogGroupName)
	log.Printf("Log Stream:       %s", logStreamName)
	log.Printf("Portfolios found: %d", len(portfolioInfo))
	log.Printf("============================================================")
}
