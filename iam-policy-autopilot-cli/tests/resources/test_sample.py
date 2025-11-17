#!/usr/bin/env python3
"""
Simple test file with AWS SDK calls for testing the IAM Policy Autopilot CLI
"""

import boto3

def main():
    # Create S3 client
    s3_client = boto3.client('s3')
    
    # Get object from S3
    response = s3_client.get_object(
        Bucket='my-test-bucket',
        Key='test-file.txt'
    )
    
    # List objects
    objects = s3_client.list_objects_v2(
        Bucket='my-test-bucket',
        Prefix='data/'
    )
    
    # Create EC2 client
    ec2_client = boto3.client('ec2')
    
    # Describe instances
    instances = ec2_client.describe_instances()
    
    print("AWS operations completed")

if __name__ == '__main__':
    main()