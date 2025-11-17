# Test script to verify our implementation works for both load and waiters
import boto3

def test_resource_operations():
    # Test case 1: table.load() should map to DescribeTable
    dynamodb = boto3.resource('dynamodb')
    table = dynamodb.Table('test-table')
    table.load()  # Should extract: describe_table with TableName=test-table
    
    # Test case 2: table.wait_until_exists() should map to DescribeTable (via TableExists waiter)
    table.wait_until_exists()  # Should extract: describe_table with TableName=test-table
    
    # Test case 3: Regular action
    response = table.get_item(Key={'id': 1})  # Should extract: get_item with TableName=test-table
    
if __name__ == "__main__":
    test_resource_operations()
