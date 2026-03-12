"""Lambda handler that interacts with S3 and DynamoDB."""
import boto3


def main(event, context):
    """Process incoming events by reading from S3 and writing to DynamoDB."""
    s3_client = boto3.client("s3")
    dynamodb = boto3.resource("dynamodb")
    sqs = boto3.client("sqs")

    # Read data from S3
    response = s3_client.get_object(Bucket="my-app-data-bucket", Key=event["key"])
    data = response["Body"].read()

    # Store processed result in DynamoDB
    table = dynamodb.Table("users-table")
    table.put_item(Item={"user_id": event["user_id"], "data": data.decode()})

    # Send notification to SQS
    sqs.send_message(
        QueueUrl="https://sqs.us-east-1.amazonaws.com/123456789012/task-processing-queue",
        MessageBody="processed",
    )

    # List objects in the logs bucket
    s3_client.list_objects_v2(Bucket="my-app-logs-bucket")

    return {"statusCode": 200}
