import boto3

def test_s3_operations():
    s3 = boto3.client('s3')
    
    # Get an object from S3
    response = s3.get_object(Bucket='my-bucket', Key='my-file.txt')
    
    # Put an object to S3
    s3.put_object(Bucket='my-bucket', Key='new-file.txt', Body=b'Hello World')
    
    # List buckets
    s3.list_buckets()

def test_dynamodb_operations():
    dynamodb = boto3.client('dynamodb')
    
    # Get an item from DynamoDB
    dynamodb.get_item(
        TableName='my-table',
        Key={'id': {'S': '123'}}
    )