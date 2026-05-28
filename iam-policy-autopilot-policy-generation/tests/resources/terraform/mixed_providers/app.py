"""App that uses S3 — tests that non-AWS providers are ignored."""
import boto3

s3 = boto3.client("s3")


def handler(event, context):
    s3.get_object(Bucket="my-s3-bucket", Key="data.txt")
    s3.put_object(Bucket="my-s3-bucket", Key="output.txt", Body=b"result")
