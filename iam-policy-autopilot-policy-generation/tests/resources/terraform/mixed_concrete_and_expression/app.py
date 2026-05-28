"""App that uses S3 — tests mixed concrete and unresolvable expression."""
import boto3

s3 = boto3.client("s3")


def handler(event, context):
    s3.get_object(Bucket="known-bucket", Key="data.txt")
    s3.put_object(Bucket="known-bucket", Key="output.txt", Body=b"result")
