package main

import (
	"context"
	"log"
	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/ec2"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"github.com/aws/aws-sdk-go-v2/service/dynamodb"
)

func main() {
	// Load AWS configuration
	cfg, err := config.LoadDefaultConfig(context.TODO())
	if err != nil {
		log.Fatal(err)
	}

	// Example 1: Basic S3 paginator pattern
	s3Client := s3.NewFromConfig(cfg)
	paginator := s3.NewListObjectsV2Paginator(s3Client, &s3.ListObjectsV2Input{
		Bucket: aws.String("my-test-bucket"),
		Prefix: aws.String("logs/"),
	})

	for paginator.HasMorePages() {
		page, err := paginator.NextPage(context.TODO())
		if err != nil {
			log.Printf("S3 ListObjectsV2 error: %v", err)
			break
		}
		log.Printf("Found %d objects in page", len(page.Contents))
	}

	// Example 2: EC2 paginator pattern
	ec2Client := ec2.NewFromConfig(cfg)
	instancePaginator := ec2.NewDescribeInstancesPaginator(ec2Client, &ec2.DescribeInstancesInput{
		MaxResults: aws.Int32(10),
	})

	for instancePaginator.HasMorePages() {
		page, err := instancePaginator.NextPage(context.TODO())
		if err != nil {
			log.Printf("EC2 DescribeInstances error: %v", err)
			break
		}
		log.Printf("Found %d reservations in page", len(page.Reservations))
	}

	// Example 3: DynamoDB paginator pattern
	dynamoClient := dynamodb.NewFromConfig(cfg)
	scanPaginator := dynamodb.NewScanPaginator(dynamoClient, &dynamodb.ScanInput{
		TableName: aws.String("my-table"),
	})

	for scanPaginator.HasMorePages() {
		page, err := scanPaginator.NextPage(context.TODO())
		if err != nil {
			log.Printf("DynamoDB Scan error: %v", err)
			break
		}
		log.Printf("Found %d items in page", len(page.Items))
	}

	// Example 4: Chained paginator call
	page, err := s3.NewListObjectsV2Paginator(s3Client, &s3.ListObjectsV2Input{
		Bucket: aws.String("another-bucket"),
		Prefix: aws.String("data/"),
	}).NextPage(context.TODO())
	
	if err != nil {
		log.Printf("Chained paginator error: %v", err)
	} else {
		log.Printf("Chained call found %d objects", len(page.Contents))
	}

	// Example 5: Multiple NextPage calls on same paginator
	multiPaginator := s3.NewListObjectsV2Paginator(s3Client, &s3.ListObjectsV2Input{
		Bucket: aws.String("multi-page-bucket"),
	})

	// First page
	page1, err := multiPaginator.NextPage(context.TODO())
	if err != nil {
		log.Printf("First page error: %v", err)
	} else {
		log.Printf("First page: %d objects", len(page1.Contents))
	}

	// Second page
	page2, err := multiPaginator.NextPage(context.TODO())
	if err != nil {
		log.Printf("Second page error: %v", err)
	} else {
		log.Printf("Second page: %d objects", len(page2.Contents))
	}

	// Example 6: Unmatched paginator (created but not used)
	unusedPaginator := ec2.NewDescribeInstancesPaginator(ec2Client, &ec2.DescribeInstancesInput{})
	_ = unusedPaginator // This should still generate a synthetic call
}