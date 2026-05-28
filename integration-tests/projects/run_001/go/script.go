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
	"github.com/aws/aws-sdk-go-v2/service/redshiftdata"
	"github.com/aws/aws-sdk-go-v2/service/sts"
)

// ── Config ────────────────────────────────────────────────────────────────────

// RunConfig mirrors the shape of config.json written by deploy.sh.
type RunConfig struct {
	BucketName                string `json:"bucketName"`
	RedshiftClusterIdentifier string `json:"redshiftClusterIdentifier"`
	Region                    string `json:"region"`
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
	log.SetPrefix("[SecurityAnalytics] ")
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

func executeRedshiftStatement(
	ctx context.Context,
	rdClient *redshiftdata.Client,
	clusterID, database, dbUser, sql string,
) (string, error) {
	result, err := rdClient.ExecuteStatement(ctx, &redshiftdata.ExecuteStatementInput{
		ClusterIdentifier: aws.String(clusterID),
		Database:          aws.String(database),
		DbUser:            aws.String(dbUser),
		Sql:               aws.String(sql),
	})
	if err != nil {
		return "", fmt.Errorf("failed to execute Redshift statement: %w", err)
	}
	stmtID := *result.Id
	log.Printf("Redshift Data API ExecuteStatement submitted, id=%s", stmtID)
	return stmtID, nil
}

func waitForRedshiftStatement(
	ctx context.Context,
	rdClient *redshiftdata.Client,
	stmtID string,
	pollInterval time.Duration,
	maxWait time.Duration,
) (string, error) {
	deadline := time.Now().Add(maxWait)
	for time.Now().Before(deadline) {
		desc, err := rdClient.DescribeStatement(ctx, &redshiftdata.DescribeStatementInput{
			Id: aws.String(stmtID),
		})
		if err != nil {
			return "", fmt.Errorf("failed to describe statement %s: %w", stmtID, err)
		}
		status := string(desc.Status)
		log.Printf("  Statement %s status: %s", stmtID, status)
		switch status {
		case "FINISHED":
			return status, nil
		case "FAILED", "ABORTED":
			errMsg := ""
			if desc.Error != nil {
				errMsg = *desc.Error
			}
			log.Printf("  Statement ended with status %s: %s", status, errMsg)
			return status, nil
		}
		time.Sleep(pollInterval)
	}
	log.Printf("  Statement %s did not finish within %s", stmtID, maxWait)
	return "TIMEOUT", nil
}

// ── Main logic ────────────────────────────────────────────────────────────────

func runSecurityAnalytics(ctx context.Context, cfg *RunConfig) error {
	log.Println("Starting AWS Security and Analytics Platform (data-plane)...")
	log.Printf("Using Redshift cluster: %s", cfg.RedshiftClusterIdentifier)
	log.Printf("Using region:           %s", cfg.Region)

	awsCfg, err := config.LoadDefaultConfig(ctx, config.WithRegion(cfg.Region))
	if err != nil {
		return fmt.Errorf("failed to load AWS config: %w", err)
	}

	stsClient := sts.NewFromConfig(awsCfg)
	rdClient  := redshiftdata.NewFromConfig(awsCfg)

	// ── STS: GetCallerIdentity ─────────────────────────────────────────────────
	accountID, err := getAWSAccountID(ctx, stsClient)
	if err != nil {
		return err
	}

	clusterID := cfg.RedshiftClusterIdentifier
	database  := "securitydb"
	dbUser    := "adminuser"

	// ── Redshift Data API: 1. CREATE TABLE ────────────────────────────────────
	log.Println("Executing Redshift statement 1/3: CREATE TABLE security_events...")
	createSQL := strings.TrimSpace(`
CREATE TABLE IF NOT EXISTS security_events (
    event_id    VARCHAR(64),
    event_type  VARCHAR(64),
    source_ip   VARCHAR(45),
    user_name   VARCHAR(128),
    timestamp   TIMESTAMP,
    severity    VARCHAR(16),
    description VARCHAR(512)
)`)

	stmtID, err := executeRedshiftStatement(ctx, rdClient, clusterID, database, dbUser, createSQL)
	if err != nil {
		return err
	}
	waitForRedshiftStatement(ctx, rdClient, stmtID, 2*time.Second, 60*time.Second)

	// ── Redshift Data API: 2. INSERT data ─────────────────────────────────────
	log.Println("Executing Redshift statement 2/3: INSERT security events...")
	insertSQL := strings.TrimSpace(`
INSERT INTO security_events
    (event_id, event_type, source_ip, user_name, timestamp, severity, description)
VALUES
    ('evt-001', 'LOGIN_FAILURE',         '192.168.1.100', 'user1', GETDATE(), 'HIGH',     'Multiple failed login attempts'),
    ('evt-002', 'DATA_ACCESS',           '10.0.0.50',     'user2', GETDATE(), 'MEDIUM',   'Unusual data access pattern'),
    ('evt-003', 'PRIVILEGE_ESCALATION',  '172.16.0.1',    'user3', GETDATE(), 'CRITICAL', 'Unauthorized privilege escalation attempt')`)

	stmtID, err = executeRedshiftStatement(ctx, rdClient, clusterID, database, dbUser, insertSQL)
	if err != nil {
		return err
	}
	waitForRedshiftStatement(ctx, rdClient, stmtID, 2*time.Second, 60*time.Second)

	// ── Redshift Data API: 3. Analytics SELECT ────────────────────────────────
	log.Println("Executing Redshift statement 3/3: Analytics query on security_events...")
	analyticsSQL := strings.TrimSpace(`
SELECT
    severity,
    COUNT(*)       AS event_count,
    MIN(timestamp) AS first_seen,
    MAX(timestamp) AS last_seen
FROM security_events
GROUP BY severity
ORDER BY event_count DESC`)

	stmtID, err = executeRedshiftStatement(ctx, rdClient, clusterID, database, dbUser, analyticsSQL)
	if err != nil {
		return err
	}
	waitForRedshiftStatement(ctx, rdClient, stmtID, 2*time.Second, 60*time.Second)

	log.Printf(strings.Repeat("=", 60))
	log.Printf("APPLICATION COMPLETED SUCCESSFULLY!")
	log.Printf(strings.Repeat("=", 60))
	log.Printf("Resources used (data-plane):")
	log.Printf("  - STS:           GetCallerIdentity (account: %s)", accountID)
	log.Printf("  - Redshift Data: ExecuteStatement x3 (cluster: %s)", clusterID)
	log.Printf(strings.Repeat("=", 60))
	log.Printf("To destroy infrastructure, run: cd ../cdk && npx cdk destroy")

	return nil
}

// ── Entry point ───────────────────────────────────────────────────────────────

func main() {
	setupLogging()
	ctx := context.Background()

	cfg, err := loadConfig()
	if err != nil {
		log.Fatalf("Configuration error: %v", err)
	}

	if err := runSecurityAnalytics(ctx, cfg); err != nil {
		log.Fatalf("Application failed: %v", err)
	}
}
