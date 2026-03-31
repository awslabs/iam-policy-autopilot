"""App with multiple tfvars files for variable override testing."""
import boto3

s3 = boto3.client("s3")
dynamodb = boto3.client("dynamodb")
sqs = boto3.client("sqs")


def handler(event, context):
    s3.get_object(Bucket="prod-data-bucket", Key="file.json")
    dynamodb.put_item(TableName="prod-items-table", Item={"item_id": {"S": "1"}})
    sqs.send_message(QueueUrl="default-queue", MessageBody="work")
