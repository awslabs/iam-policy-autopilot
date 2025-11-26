"""
Test file to demonstrate boto3 waiter extraction.
"""

import boto3

def test_waiters(dynamodb_client, table_name):
    # Create EC2 client
    ec2_client = boto3.client('ec2', region_name='us-west-2')
    
    # Example 1: Matched waiter + wait call
    instance_id = 'i-1234567890abcdef0'
    waiter = ec2_client.get_waiter('instance_terminated')
    waiter.wait(InstanceIds=[instance_id], WaiterConfig={'Delay': 15, 'MaxAttempts': 20})
    
    # Example 2: Unmatched get_waiter (no wait call)
    # Should generate synthetic call with required params
    unmatched_waiter = ec2_client.get_waiter('instance_running')
    
    # Example 3: Multiple waiter types
    s3_client = boto3.client('s3')
    bucket_waiter = s3_client.get_waiter('bucket_exists')
    bucket_waiter.wait(Bucket='my-test-bucket')

    # Example 4: Chained call (without client information in scope)
    dynamodb_client.get_waiter('table_exists').wait(TableName=table_name)

if __name__ == "__main__":
    test_waiters()
