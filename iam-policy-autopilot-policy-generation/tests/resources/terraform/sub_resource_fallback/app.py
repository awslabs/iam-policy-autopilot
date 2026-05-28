"""App that uses S3 — tests sub-resource (object) fallback to parent bucket."""
import boto3

s3 = boto3.client("s3")


def handler(event, context):
    s3.get_object(Bucket="my-bucket", Key="data.txt")
    s3.put_object(Bucket="my-bucket", Key="output.txt", Body=b"result")
