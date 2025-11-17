#!/usr/bin/env python3
"""
Test file to demonstrate the arg_ prefix collision fix.

This file contains method calls with keyword arguments that start with "arg_"
to verify that they are no longer incorrectly filtered out.
"""

import boto3

def test_arg_prefix_collision_fix():
    """
    Test case demonstrating keyword arguments starting with "arg_" prefix.
    
    Before the fix: These would be incorrectly filtered out due to name-based filtering.
    After the fix: These are correctly identified as keyword parameters and validated properly.
    """
    
    # Example 1: S3 client with a hypothetical keyword argument starting with "arg_"
    s3_client = boto3.client('s3')
    
    # This call has legitimate parameters plus a keyword argument starting with "arg_"
    # The disambiguation should now correctly identify this as a keyword parameter
    # and validate it against the AWS API (it will be rejected for being invalid, not for the prefix)
    result1 = s3_client.get_object(
        Bucket='my-bucket',
        Key='my-key',
        arg_custom_setting='some_value'  # This starts with "arg_" but is a keyword argument
    )
    
    # Example 2: Mixed positional and keyword arguments
    # The positional arguments should still be filtered out during validation
    # but keyword arguments starting with "arg_" should be processed
    result2 = s3_client.list_objects_v2(
        'my-bucket',  # Positional argument (will get arg_0 name and Positional type)
        Prefix='folder/',  # Keyword argument
        arg_filter_option='enabled'  # Keyword argument starting with "arg_"
    )
    
    # Example 3: API Gateway V2 client
    apigateway_client = boto3.client('apigatewayv2')
    
    result3 = apigateway_client.create_api_mapping(
        DomainName='example.com',
        Stage='prod',
        ApiId='abc123',
        arg_deployment_config='standard'  # Another keyword argument starting with "arg_"
    )

if __name__ == '__main__':
    test_arg_prefix_collision_fix()