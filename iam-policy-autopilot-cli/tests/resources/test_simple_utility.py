from datetime import datetime
import boto3
import io

# =============================================================================
# S3 UTILITY METHODS - Client Level
# =============================================================================
def test_s3_client_methods():
    """Test S3 client-level utility methods"""
    s3_client = boto3.client('s3')
    
    s3_client.upload_file('/tmp/test.txt', 'my-bucket', 'my-key')
    
    s3_client.download_file('my-bucket5', 'my-key5', '/tmp/output.txt')
    
    s3_client.copy({'Bucket': 'source-bucket', 'Key': 'source-key'}, 'dest-bucket', 'dest-key')
    
    file_obj = io.BytesIO(b'test data')
    s3_client.upload_fileobj(file_obj, 'my-bucket', 'my-key')
    
    output_obj = io.BytesIO()
    s3_client.download_fileobj('my-bucket', 'my-key', output_obj)


# # =============================================================================
# # S3 UTILITY METHODS - Resource Level
# # =============================================================================
def test_s3_resource_methods():
    """Test S3 resource-level utility methods and delegate methods"""
    s3 = boto3.resource('s3')
    
    bucket = s3.Bucket('my-bucket1')
    bucket.load()
    
    bucket.upload_file('/tmp/test.txt', 'my-key1')
    
    bucket.download_file('my-key', '/tmp/output.txt')
    
    bucket.copy({'Bucket': 'source-bucket', 'Key': 'source-key'}, 'dest-key')
    
    file_obj = io.BytesIO(b'test data')
    bucket.upload_fileobj(file_obj, 'my-key')
    
    output_obj = io.BytesIO()
    bucket.download_fileobj('my-key', output_obj)
    
    obj_summary = s3.ObjectSummary('my-bucket', 'my-key')
    obj_summary.load()
    
    # Object delegate methods
    obj = s3.Object('my-bucket2', 'my-key2')
    obj.upload_file('/tmp/test.txt')
    obj.download_file('/tmp/output.txt')
    obj.copy({'Bucket': 'source-bucket', 'Key': 'source-key'})
    obj.upload_fileobj(io.BytesIO(b'data'))
    obj.download_fileobj(io.BytesIO())


# =============================================================================
# EC2 UTILITY METHODS
# =============================================================================
def test_ec2_methods():
    """Test EC2 utility methods"""
    ec2 = boto3.resource('ec2')
    
    # create_tags - create tags on EC2 resources
    ec2.create_tags(
        Resources=['i-1234567890abcdef0', 'vol-049df61146c4d7901'],
        Tags=[
            {'Key': 'Environment', 'Value': 'Production'},
            {'Key': 'Application', 'Value': 'WebServer'}
        ]
    )
    
    # delete_tags - delete tags from resources
    tag = ec2.Tag('i-1234567890abcdef0', 'Environment', 'Production')
    tag.delete(Resources=['i-1234567890abcdef0'])


# =============================================================================
# DYNAMODB UTILITY METHODS
# =============================================================================
def test_dynamodb_methods():
    """Test DynamoDB utility methods"""
    dynamodb = boto3.resource('dynamodb')
    table = dynamodb.Table('my-table')
    
    # batch_writer - BatchWriter context manager that calls batch_write_item
    with table.batch_writer() as batch:
        batch.put_item(Item={'id': '1', 'data': 'value1'})
        batch.put_item(Item={'id': '2', 'data': 'value2'})


# =============================================================================
# UNMATCHED RESOURCE OBJECTS -> fallback conservative
# =============================================================================
def test_error_cases():
    """Test cases that should NOT be expanded due to missing required parameters"""
    s3_client = boto3.client('s3')
    
    s3_client.upload_file('/tmp/file.txt')  # Missing Bucket and Key
    s3_client.download_file('my-bucket')     # Missing Key and Filename
    s3_client.copy({'Bucket': 'src'})        # Missing Key in CopySource


# =============================================================================
# Execute all test functions
# =============================================================================
test_s3_client_methods()
test_s3_resource_methods()
test_ec2_methods()
test_dynamodb_methods()
test_error_cases()
