"""App with resources split across multiple .tf files."""
import boto3

s3 = boto3.client("s3")
dynamodb = boto3.client("dynamodb")
sqs = boto3.client("sqs")


def handler(event, context):
    s3.get_object(Bucket="my-data-bucket", Key="test.json")
    dynamodb.put_item(TableName="items-table", Item={"item_id": {"S": "1"}})
    sqs.send_message(QueueUrl="task-queue", MessageBody="work")
