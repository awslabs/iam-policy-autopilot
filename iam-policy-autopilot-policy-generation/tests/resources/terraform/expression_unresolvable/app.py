"""App that uses S3 — tests unresolvable var expressions."""
import boto3

s3 = boto3.client("s3")


def handler(event, context):
    s3.get_object(Bucket="some-bucket", Key="data.txt")
