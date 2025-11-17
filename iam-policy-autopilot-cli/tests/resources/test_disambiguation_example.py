#!/usr/bin/env python3
"""
Test file to demonstrate method disambiguation functionality.

This file contains various method calls that should be disambiguated:
- Valid AWS SDK calls that should be kept
- Invalid AWS SDK calls that should be filtered out
- Non-AWS method calls that should be filtered out
- Dictionary unpacking cases that should be kept for future analysis
"""

import boto3
from typing import Dict, Any

class CustomClient:
    """A custom client class to simulate non-AWS method calls."""
    
    def create_api_mapping(self, custom_param: str) -> Dict[str, Any]:
        """Custom method with same name as AWS but different parameters."""
        return {"result": custom_param}
    
    def get_object(self, custom_param: str) -> Dict[str, Any]:
        """Custom method with same name as AWS but different parameters."""
        return {"data": custom_param}
    
    def custom_method(self, param: str) -> str:
        """Completely custom method."""
        return f"custom: {param}"

def valid_aws_sdk_calls():
    """Examples of valid AWS SDK calls that should be kept."""
    
    # Valid API Gateway V2 call with all required parameters
    apigateway_client = boto3.client('apigatewayv2')
    result1 = apigateway_client.create_api_mapping(
        DomainName='example.com',
        Stage='prod',
        ApiId='abc123'
    )
    
    # Valid API Gateway V2 call with optional parameter
    result2 = apigateway_client.create_api_mapping(
        DomainName='example.com',
        Stage='prod', 
        ApiId='abc123',
        ApiMappingKey='v1'  # Optional parameter
    )
    
    # Valid S3 call with required parameters
    s3_client = boto3.client('s3')
    result3 = s3_client.get_object(
        Bucket='my-bucket',
        Key='my-key'
    )
    
    # Valid S3 call with optional parameter
    result4 = s3_client.get_object(
        Bucket='my-bucket',
        Key='my-key',
        VersionId='version123'  # Optional parameter
    )
    
    base_params = {'DomainName': 'example.com'}
    extra_params = {'Stage': 'prod'}
    result5 = apigateway_client.create_api_mapping(
        **base_params,
        **extra_params,
        ApiMappingKey='v1'
    )

def invalid_aws_sdk_calls():
    """Examples of invalid AWS SDK calls that should be filtered out."""
    
    apigateway_client = boto3.client('apigatewayv2')
    
    # Missing required parameters - should be filtered out
    try:
        result1 = apigateway_client.create_api_mapping(
            DomainName='example.com'
            # Missing required Stage and ApiId
        )
    except:
        pass
    
    # Invalid parameter - should be filtered out
    try:
        result2 = apigateway_client.create_api_mapping(
            DomainName='example.com',
            Stage='prod',
            ApiId='abc123',
            InvalidParam='invalid'  # This parameter doesn't exist in AWS API
        )
    except:
        pass
    
    s3_client = boto3.client('s3')
    
    # Missing required Key parameter - should be filtered out
    try:
        result3 = s3_client.get_object(
            Bucket='my-bucket'
            # Missing required Key parameter
        )
    except:
        pass

def dictionary_unpacking_calls():
    """Examples of dictionary unpacking that should be kept for future analysis."""
    
    apigateway_client = boto3.client('apigatewayv2')
    s3_client = boto3.client('s3')
    
    # Dictionary unpacking - can't validate statically, so keep for future analysis
    api_params = {
        'DomainName': 'example.com',
        'Stage': 'prod',
        'ApiId': 'abc123'
    }
    result1 = apigateway_client.create_api_mapping(**api_params)
    
    # Another dictionary unpacking example
    s3_params = {
        'Bucket': 'my-bucket',
        'Key': 'my-key'
    }
    result2 = s3_client.get_object(**s3_params)
    
    # Mixed parameters and unpacking
    result3 = s3_client.get_object(
        Bucket='my-bucket',
        **{'Key': 'my-key', 'VersionId': 'version123'}
    )

def non_aws_method_calls():
    """Examples of non-AWS method calls that should be filtered out."""
    
    custom_client = CustomClient()
    
    # Same method name as AWS but different parameters - should be filtered out
    result1 = custom_client.create_api_mapping(custom_param='value')
    
    # Same method name as AWS but different parameters - should be filtered out  
    result2 = custom_client.get_object(custom_param='data')
    
    # Completely different method - should be filtered out
    result3 = custom_client.custom_method(param='test')
    
    # Regular Python method calls - should be filtered out
    my_list = [1, 2, 3]
    result4 = my_list.append(4)
    
    my_dict = {'key': 'value'}
    result5 = my_dict.get('key', 'default')

def conditional_parameters():
    """Examples with conditional parameters."""
    
    s3_client = boto3.client('s3')
    
    # Conditional parameter building
    params = {'Bucket': 'my-bucket', 'Key': 'my-key'}
    
    version_id = get_version_id()  # Hypothetical function
    if version_id:
        params['VersionId'] = version_id
    
    # This should be kept for future analysis due to dictionary unpacking
    result = s3_client.get_object(**params)

def get_version_id():
    """Hypothetical helper function."""
    return 'version123'

if __name__ == '__main__':
    print("This file demonstrates various method call patterns for disambiguation testing.")
    print("Run the IAM Policy Autopilot extraction tool on this file to see disambiguation in action.")