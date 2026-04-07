"""App that uses S3, DynamoDB, and SQS with variables."""
import boto3

s3 = boto3.client("s3")
dynamodb = boto3.client("dynamodb")
sqs = boto3.client("sqs")


def handler(event, context):
    s3.put_object(Bucket="myapp-prod-data", Key="test", Body=b"data")
    dynamodb.get_item(TableName="users-prod", Key={"user_id": {"S": "123"}})
    sqs.send_message(QueueUrl="myapp-tasks", MessageBody="task")
