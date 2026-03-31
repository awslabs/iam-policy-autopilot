"""App that uses S3, DynamoDB, SQS, Lambda, and IAM — state precedence test."""
import boto3

s3 = boto3.client("s3")
dynamodb = boto3.client("dynamodb")
sqs = boto3.client("sqs")
lam = boto3.client("lambda")
iam = boto3.client("iam")


def handler(event, context):
    s3.get_object(Bucket="my-app-data-bucket", Key="test")
    s3.put_object(Bucket="my-app-logs-bucket", Key="log.txt", Body=b"data")
    dynamodb.get_item(TableName="users-table", Key={"user_id": {"S": "123"}})
    sqs.send_message(QueueUrl="task-processing-queue", MessageBody="task")
    lam.invoke(FunctionName="data-processor")
    iam.get_role(RoleName="data-processor-role")
