import json
import boto3
import logging
import uuid
from datetime import datetime
import os
from botocore.exceptions import ClientError

# Configure logging
logger = logging.getLogger()
logger.setLevel(logging.INFO)

# Initialize AWS clients
s3_client = boto3.client('s3')

def lambda_handler(event, context):
    """
    Lambda function to get data from input and upload to S3.
    Expects data in the event payload to upload to S3.
    """
    try:
        # Get environment variables
        bucket_name = os.environ.get('S3_BUCKET', 'default-bucket')
        
        # Extract data from lambda input event
        if 'data' not in event:
            raise ValueError("No 'data' field found in event payload")
        
        data = event['data']
        logger.info(f"Received data to upload: {type(data).__name__} with length/size: {len(str(data))}")
        
        # Generate unique key for S3 object
        timestamp = datetime.utcnow().strftime('%Y%m%d_%H%M%S')
        unique_id = str(uuid.uuid4())[:8]
        
        # Determine file extension based on data type
        if isinstance(data, (dict, list)):
            file_extension = 'json'
            content = json.dumps(data, indent=2)
            content_type = 'application/json'
        else:
            file_extension = 'txt'
            content = str(data)
            content_type = 'text/plain'
        
        # Create S3 key
        s3_key = f"uploads/{timestamp}_{unique_id}.{file_extension}"
        
        # Upload data to S3
        try:
            s3_client.put_object(
                Bucket=bucket_name,
                Key=s3_key,
                Body=content,
                ContentType=content_type
            )
            
            # Log successful upload to CloudWatch
            logger.info(f"Successfully uploaded data to S3: s3://{bucket_name}/{s3_key}")
            logger.info(f"Upload details - Size: {len(content)} bytes, Type: {content_type}")
            
        except ClientError as e:
            logger.error(f"Error uploading to S3: {e}")
            raise Exception(f"Failed to upload to S3: {str(e)}")
        
        # Prepare successful response
        response = {
            'statusCode': 200,
            'body': json.dumps({
                'message': 'Data successfully uploaded to S3',
                'bucket': bucket_name,
                's3_key': s3_key,
                'upload_size': len(content),
                'content_type': content_type,
                'timestamp': datetime.utcnow().isoformat()
            })
        }
        
        return response
        
    except Exception as e:
        logger.error(f"Error processing upload request: {str(e)}")
        return {
            'statusCode': 500,
            'body': json.dumps({
                'message': 'Error uploading data to S3',
                'error': str(e)
            })
        }
