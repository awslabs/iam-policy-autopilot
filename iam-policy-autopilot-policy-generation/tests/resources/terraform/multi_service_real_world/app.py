"""App using Kinesis, SNS, S3, DynamoDB, Lambda, and IAM — real-world multi-service."""
import boto3

kinesis = boto3.client("kinesis")
sns = boto3.client("sns")
s3 = boto3.client("s3")
dynamodb = boto3.client("dynamodb")
lam = boto3.client("lambda")
iam = boto3.client("iam")


def handler(event, context):
    kinesis.put_record(StreamName="app-events-stream", Data=b"event", PartitionKey="pk")
    sns.publish(TopicArn="arn:aws:sns:us-east-1:123456789012:app-notifications", Message="hello")
    s3.get_object(Bucket="app-event-archive", Key="data.txt")
    s3.put_object(Bucket="app-staging-data", Key="stage.txt", Body=b"staged")
    s3.get_object(Bucket="app-results", Key="result.txt")
    dynamodb.get_item(TableName="events-log", Key={"event_id": {"S": "e1"}})
    lam.invoke(FunctionName="event-processor")
    iam.get_role(RoleName="event-processor-role")
