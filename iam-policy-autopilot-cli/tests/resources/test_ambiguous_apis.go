package main

import (
	"context"
	"fmt"
	"log"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/dynamodb"
	"github.com/aws/aws-sdk-go-v2/service/dynamodb/types"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"github.com/aws/aws-sdk-go-v2/service/secretsmanager"
	"github.com/aws/aws-sdk-go-v2/service/lambda"
	"github.com/aws/aws-sdk-go-v2/service/cloudwatchlogs"
	"github.com/aws/aws-sdk-go-v2/service/organizations"
	"github.com/aws/aws-sdk-go-v2/service/ssm"
	"github.com/aws/aws-sdk-go-v2/service/xray"
	"github.com/aws/aws-sdk-go-v2/service/glue"
	"github.com/aws/aws-sdk-go-v2/service/ec2"
)

// This test file contains highly ambiguous AWS API calls that exist across multiple services
// and should challenge the disambiguation logic significantly
func main() {
	ctx := context.Background()
	
	// Load AWS configuration
	cfg, err := config.LoadDefaultConfig(ctx, config.WithRegion("us-east-1"))
	if err != nil {
		log.Fatalf("unable to load SDK config, %v", err)
	}

	// Test Case 1: PutResourcePolicy - exists in 50+ services
	// This is one of the most ambiguous APIs in AWS
	testPutResourcePolicyAmbiguity(ctx, cfg)
	
	// Test Case 2: GetResourcePolicy - also highly ambiguous
	testGetResourcePolicyAmbiguity(ctx, cfg)
	
	// Test Case 3: DeleteResourcePolicy - another common ambiguous API
	testDeleteResourcePolicyAmbiguity(ctx, cfg)
	
	// Test Case 4: Mixed service calls with similar parameter structures
	testMixedServiceCalls(ctx, cfg)
	
	// Test Case 5: Complex nested struct literals with ambiguous field names
	testComplexStructLiterals(ctx, cfg)
	
	// Test Case 6: Method calls with minimal context clues
	testMinimalContextClues(ctx, cfg)
}

func testPutResourcePolicyAmbiguity(ctx context.Context, cfg aws.Config) {
	// PutResourcePolicy exists in: DynamoDB, S3, SecretsManager, Lambda, CloudWatch Logs,
	// Organizations, SSM, X-Ray, Glue, EC2, and many more services
	
	// DynamoDB PutResourcePolicy
	dynamoClient := dynamodb.NewFromConfig(cfg)
	_, err := dynamoClient.PutResourcePolicy(ctx, &dynamodb.PutResourcePolicyInput{
		ResourceArn: aws.String("arn:aws:dynamodb:us-east-1:123456789012:table/MyTable"),
		Policy: aws.String(`{
			"Version": "2012-10-17",
			"Statement": [{
				"Effect": "Allow",
				"Principal": {"AWS": "arn:aws:iam::123456789012:root"},
				"Action": "dynamodb:GetItem",
				"Resource": "*"
			}]
		}`),
	})
	if err != nil {
		log.Printf("DynamoDB PutResourcePolicy error: %v", err)
	}

	// SecretsManager PutResourcePolicy - very similar structure
	secretsClient := secretsmanager.NewFromConfig(cfg)
	_, err = secretsClient.PutResourcePolicy(ctx, &secretsmanager.PutResourcePolicyInput{
		SecretId: aws.String("MySecret"),
		ResourcePolicy: aws.String(`{
			"Version": "2012-10-17",
			"Statement": [{
				"Effect": "Allow",
				"Principal": {"AWS": "arn:aws:iam::123456789012:root"},
				"Action": "secretsmanager:GetSecretValue",
				"Resource": "*"
			}]
		}`),
		BlockPublicPolicy: aws.Bool(true),
	})
	if err != nil {
		log.Printf("SecretsManager PutResourcePolicy error: %v", err)
	}

	// Lambda PutResourcePolicy - different parameter names but same concept
	lambdaClient := lambda.NewFromConfig(cfg)
	_, err = lambdaClient.AddPermission(ctx, &lambda.AddPermissionInput{
		FunctionName: aws.String("MyFunction"),
		StatementId:  aws.String("allow-s3"),
		Action:       aws.String("lambda:InvokeFunction"),
		Principal:    aws.String("s3.amazonaws.com"),
		SourceArn:    aws.String("arn:aws:s3:::my-bucket/*"),
	})
	if err != nil {
		log.Printf("Lambda AddPermission error: %v", err)
	}

	// CloudWatch Logs PutResourcePolicy
	logsClient := cloudwatchlogs.NewFromConfig(cfg)
	_, err = logsClient.PutResourcePolicy(ctx, &cloudwatchlogs.PutResourcePolicyInput{
		PolicyName:     aws.String("MyLogPolicy"),
		PolicyDocument: aws.String(`{
			"Version": "2012-10-17",
			"Statement": [{
				"Effect": "Allow",
				"Principal": {"Service": "vpc-flow-logs.amazonaws.com"},
				"Action": "logs:CreateLogStream",
				"Resource": "*"
			}]
		}`),
	})
	if err != nil {
		log.Printf("CloudWatch Logs PutResourcePolicy error: %v", err)
	}

	// Organizations PutResourcePolicy
	orgsClient := organizations.NewFromConfig(cfg)
	_, err = orgsClient.PutResourcePolicy(ctx, &organizations.PutResourcePolicyInput{
		Content: aws.String(`{
			"Version": "2012-10-17",
			"Statement": [{
				"Effect": "Allow",
				"Principal": {"AWS": "arn:aws:iam::123456789012:root"},
				"Action": "organizations:DescribeOrganization",
				"Resource": "*"
			}]
		}`),
	})
	if err != nil {
		log.Printf("Organizations PutResourcePolicy error: %v", err)
	}
}

func testGetResourcePolicyAmbiguity(ctx context.Context, cfg aws.Config) {
	// GetResourcePolicy is also highly ambiguous across services
	
	// SSM GetResourcePolicies (note: plural form adds complexity)
	ssmClient := ssm.NewFromConfig(cfg)
	_, err := ssmClient.GetResourcePolicies(ctx, &ssm.GetResourcePoliciesInput{
		ResourceArn: aws.String("arn:aws:ssm:us-east-1:123456789012:parameter/MyParam"),
	})
	if err != nil {
		log.Printf("SSM GetResourcePolicies error: %v", err)
	}

	// X-Ray GetSamplingRules
	xrayClient := xray.NewFromConfig(cfg)
	_, err = xrayClient.GetSamplingRules(ctx, &xray.GetSamplingRulesInput{})
	if err != nil {
		log.Printf("X-Ray GetSamplingRules error: %v", err)
	}

	// Glue GetResourcePolicy
	glueClient := glue.NewFromConfig(cfg)
	_, err = glueClient.GetResourcePolicy(ctx, &glue.GetResourcePolicyInput{
		ResourceArn: aws.String("arn:aws:glue:us-east-1:123456789012:catalog"),
	})
	if err != nil {
		log.Printf("Glue GetResourcePolicy error: %v", err)
	}
}

func testDeleteResourcePolicyAmbiguity(ctx context.Context, cfg aws.Config) {
	// DeleteResourcePolicy across multiple services
	
	// DynamoDB DeleteResourcePolicy
	dynamoClient := dynamodb.NewFromConfig(cfg)
	_, err := dynamoClient.DeleteResourcePolicy(ctx, &dynamodb.DeleteResourcePolicyInput{
		ResourceArn:     aws.String("arn:aws:dynamodb:us-east-1:123456789012:table/MyTable"),
		ExpectedRevisionId: aws.String("12345"),
	})
	if err != nil {
		log.Printf("DynamoDB DeleteResourcePolicy error: %v", err)
	}

	// SecretsManager DeleteResourcePolicy
	secretsClient := secretsmanager.NewFromConfig(cfg)
	_, err = secretsClient.DeleteResourcePolicy(ctx, &secretsmanager.DeleteResourcePolicyInput{
		SecretId: aws.String("MySecret"),
	})
	if err != nil {
		log.Printf("SecretsManager DeleteResourcePolicy error: %v", err)
	}
}

func testMixedServiceCalls(ctx context.Context, cfg aws.Config) {
	// Mix of services with similar method names and parameter structures
	
	// S3 operations
	s3Client := s3.NewFromConfig(cfg)
	_, err := s3Client.GetObject(ctx, &s3.GetObjectInput{
		Bucket: aws.String("my-bucket"),
		Key:    aws.String("my-key"),
		Range:  aws.String("bytes=0-1023"),
	})
	if err != nil {
		log.Printf("S3 GetObject error: %v", err)
	}

	// DynamoDB operations with similar parameter names
	dynamoClient := dynamodb.NewFromConfig(cfg)
	_, err = dynamoClient.GetItem(ctx, &dynamodb.GetItemInput{
		TableName: aws.String("MyTable"),
		Key: map[string]types.AttributeValue{
			"id": &types.AttributeValueMemberS{Value: "123"},
		},
		ConsistentRead: aws.Bool(true),
	})
	if err != nil {
		log.Printf("DynamoDB GetItem error: %v", err)
	}

	// EC2 operations
	ec2Client := ec2.NewFromConfig(cfg)
	_, err = ec2Client.DescribeInstances(ctx, &ec2.DescribeInstancesInput{
		InstanceIds: []string{"i-1234567890abcdef0"},
	})
	if err != nil {
		log.Printf("EC2 DescribeInstances error: %v", err)
	}
}

func testComplexStructLiterals(ctx context.Context, cfg aws.Config) {
	// Complex nested structures that could be ambiguous
	
	dynamoClient := dynamodb.NewFromConfig(cfg)
	
	// Complex DynamoDB query with nested attribute values
	_, err := dynamoClient.Query(ctx, &dynamodb.QueryInput{
		TableName: aws.String("ComplexTable"),
		IndexName: aws.String("GSI1"),
		KeyConditionExpression: aws.String("pk = :pk AND begins_with(sk, :sk_prefix)"),
		FilterExpression: aws.String("attribute_exists(#attr1) AND #attr2 > :val"),
		ExpressionAttributeNames: map[string]string{
			"#attr1": "status",
			"#attr2": "timestamp",
		},
		ExpressionAttributeValues: map[string]types.AttributeValue{
			":pk": &types.AttributeValueMemberS{Value: "USER#123"},
			":sk_prefix": &types.AttributeValueMemberS{Value: "ORDER#"},
			":val": &types.AttributeValueMemberN{Value: "1640995200"},
		},
		Limit: aws.Int32(50),
		ScanIndexForward: aws.Bool(false),
		Select: "ALL_ATTRIBUTES",
		ConsistentRead: aws.Bool(false),
	})
	if err != nil {
		log.Printf("DynamoDB Query error: %v", err)
	}

	// Complex batch operations
	_, err = dynamoClient.BatchWriteItem(ctx, &dynamodb.BatchWriteItemInput{
		RequestItems: map[string][]types.WriteRequest{
			"Table1": {
				{
					PutRequest: &types.PutRequest{
						Item: map[string]types.AttributeValue{
							"id": &types.AttributeValueMemberS{Value: "item1"},
							"data": &types.AttributeValueMemberM{
								Value: map[string]types.AttributeValue{
									"nested": &types.AttributeValueMemberS{Value: "value"},
									"list": &types.AttributeValueMemberL{
										Value: []types.AttributeValue{
											&types.AttributeValueMemberN{Value: "1"},
											&types.AttributeValueMemberN{Value: "2"},
										},
									},
								},
							},
						},
					},
				},
				{
					DeleteRequest: &types.DeleteRequest{
						Key: map[string]types.AttributeValue{
							"id": &types.AttributeValueMemberS{Value: "item2"},
						},
					},
				},
			},
		},
		ReturnConsumedCapacity: "TOTAL",
		ReturnItemCollectionMetrics: "SIZE",
	})
	if err != nil {
		log.Printf("DynamoDB BatchWriteItem error: %v", err)
	}
}

func testMinimalContextClues(ctx context.Context, cfg aws.Config) {
	// Test cases with minimal context that make disambiguation challenging
	
	// Generic client variable names
	client1 := dynamodb.NewFromConfig(cfg)
	client2 := s3.NewFromConfig(cfg)
	client3 := secretsmanager.NewFromConfig(cfg)
	
	// Method calls with minimal distinguishing features
	result1, _ := client1.DescribeTable(ctx, &dynamodb.DescribeTableInput{
		TableName: aws.String("MyTable"),
	})
	fmt.Printf("Table status: %v\n", result1)
	
	result2, _ := client2.HeadBucket(ctx, &s3.HeadBucketInput{
		Bucket: aws.String("my-bucket"),
	})
	fmt.Printf("Bucket exists: %v\n", result2)
	
	result3, _ := client3.DescribeSecret(ctx, &secretsmanager.DescribeSecretInput{
		SecretId: aws.String("MySecret"),
	})
	fmt.Printf("Secret ARN: %v\n", result3)
	
	// Chained method calls that could be ambiguous
	if table, err := client1.DescribeTable(ctx, &dynamodb.DescribeTableInput{
		TableName: aws.String("MyTable"),
	}); err == nil {
		// Use table info for another call
		_, _ = client1.UpdateTable(ctx, &dynamodb.UpdateTableInput{
			TableName: table.Table.TableName,
			BillingMode: "PAY_PER_REQUEST",
		})
	}
	
	// Anonymous function with method calls
	func() {
		anonymousClient := lambda.NewFromConfig(cfg)
		_, _ = anonymousClient.ListFunctions(ctx, &lambda.ListFunctionsInput{
			MaxItems: aws.Int32(10),
		})
	}()
}

// Helper function that could be confused with AWS SDK methods
func PutResourcePolicy(resourceArn, policy string) error {
	// This is NOT an AWS SDK call but could confuse the extractor
	log.Printf("Custom PutResourcePolicy called with ARN: %s", resourceArn)
	return nil
}

// Another helper that mimics AWS patterns
func GetResourcePolicy(ctx context.Context, resourceArn string) (string, error) {
	// This is also NOT an AWS SDK call
	return "", fmt.Errorf("not implemented")
}