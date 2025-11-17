package main

import (
    "context"
    "log"
    "time"
    "github.com/aws/aws-sdk-go-v2/aws"
    "github.com/aws/aws-sdk-go-v2/config"
    "github.com/aws/aws-sdk-go-v2/service/ec2"
    "github.com/aws/aws-sdk-go-v2/service/s3"
)

func main() {
    // Load AWS configuration
    cfg, err := config.LoadDefaultConfig(context.TODO())
    if err != nil {
        log.Fatal(err)
    }

    // Create EC2 client
    ec2Client := ec2.NewFromConfig(cfg)
    
    // Example 1: EC2 instance operations (simulating waiter pattern)
    _, err = ec2Client.DescribeInstances(context.TODO(), &ec2.DescribeInstancesInput{
        InstanceIds: []string{"i-1234567890abcdef0"},
    })
    if err != nil {
        log.Printf("EC2 DescribeInstances error: %v", err)
    }
    
    // Example 2: Another EC2 operation
    _, err = ec2Client.DescribeInstances(context.TODO(), &ec2.DescribeInstancesInput{
        InstanceIds: []string{"i-0987654321fedcba0"},
    })
    if err != nil {
        log.Printf("EC2 DescribeInstances error: %v", err)
    }
    
    // Example 3: S3 bucket operations (simulating waiter pattern)
    s3Client := s3.NewFromConfig(cfg)
    _, err = s3Client.HeadBucket(context.TODO(), &s3.HeadBucketInput{
        Bucket: aws.String("my-test-bucket"),
    })
    if err != nil {
        log.Printf("S3 HeadBucket error: %v", err)
    }
    
    // Simulate polling behavior
    time.Sleep(1 * time.Second)
}