#!/usr/bin/env python3
"""
Test script to demonstrate dictionary unpacking functionality
with the refactored Parameter-based approach.
"""

import boto3

def test_dictionary_unpacking():
    # Create S3 client
    s3_client = boto3.client('s3')
    
    # Test case 1: Dictionary unpacking with **kwargs
    params = {
        'Bucket': 'my-test-bucket',
        'Key': 'test-file.txt'
    }
    result1 = s3_client.get_object(**params)
    
    # Test case 2: Mixed explicit parameters and dictionary unpacking
    additional_params = {
        'VersionId': 'v123',
        'Range': 'bytes=0-1023'
    }
    result2 = s3_client.get_object(
        Bucket='my-test-bucket',
        Key='test-file.txt',
        **additional_params
    )
    
    # Test case 3: Multiple dictionary unpacking
    base_params = {'Bucket': 'my-test-bucket'}
    extra_params = {'Key': 'test-file.txt', 'VersionId': 'v456'}
    result3 = s3_client.get_object(**base_params, **extra_params)

if __name__ == '__main__':
    test_dictionary_unpacking()