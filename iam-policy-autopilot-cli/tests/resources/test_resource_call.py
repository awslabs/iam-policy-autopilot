from botocore.exceptions import ClientError, ParamValidationError
import boto3

# boto3_provider initialized in another file

def get_config_db(table_name: str, _region: str):
    dynamodb = boto3_provider.get_service_resource("dynamodb")
    table = dynamodb.Table(table_name)
    try:
        response = table.get_item(
            Key={
                'aws_region': _region
            }
        )
        if 'Item' in response:
            return response['Item']
        else:
            response = table.get_item(
                Key={
                    'aws_region': 'us-west-2'
                }
            )
        if 'Item' in response:
            return response['Item']
    except ClientError as e:
        raise e
    except ClientError as e:
        raise e
    except ParamValidationError as e:
        raise ValueError(
            'The parameters you provided are incorrect: {}'.format(e))
    
def s3_bucket_resource():
    # Replace 'your-bucket-name' with the actual name of your S3 bucket
    bucket_name = 'your-bucket-name'

    # Create an S3 resource object
    s3 = boto3.resource('s3')

    # Get a reference to the specific bucket
    bucket = s3.Bucket(bucket_name)

    print(f"\nObjects in bucket '{bucket_name}':")
    # Iterate through all objects in the bucket and print their keys
    for obj in bucket.objects.all():
        print(obj.key)

def s3_upload_to_bucket():
    # Replace with your bucket name, local file path, and desired S3 key
    bucket_name = 'your-bucket-name'
    local_file_path = 'path/to/your/local/file.txt'
    s3_object_key = 'folder/file_in_s3.txt'

    s3 = boto3.resource('s3')
    bucket = s3.Bucket(bucket_name)

    try:
        bucket.upload_file(local_file_path, s3_object_key)
        print(f"File '{local_file_path}' uploaded to '{s3_object_key}' in bucket '{bucket_name}'.")
    except Exception as e:
        print(f"Error uploading file: {e}")