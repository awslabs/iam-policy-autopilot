"""App that uses S3 and DynamoDB."""
import boto3

s3 = boto3.client("s3")
dynamodb = boto3.resource("dynamodb")

def handler(event, context):
    s3.put_object(Bucket="myapp-prod-data", Key="test", Body=b"data")
    table = dynamodb.Table("users-prod")
    table.get_item(Key={"user_id": "123"})
