import boto3

dynamodb = boto3.resource('dynamodb')
table = dynamodb.Table('my-table')
response = table.get_item(Key={'id': '123'})

s3 = boto3.resource('s3')
bucket = s3.Bucket('my-bucket')
bucket.delete()
