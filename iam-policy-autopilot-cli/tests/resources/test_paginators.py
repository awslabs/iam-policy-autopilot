import boto3

def common_patterns():
    # Create a client
    client = boto3.client('s3', region_name='us-west-2')

    # Create a reusable Paginator
    paginator = client.get_paginator('list_objects_v2')

    # Create a PageIterator from the Paginator
    page_iterator = paginator.paginate(Bucket='amzn-s3-demo-bucket')

    for page in page_iterator:
        print(page['Contents'])

    paginator = client.get_paginator('list_objects_v2')
    page_iterator = paginator.paginate(Bucket='amzn-s3-demo-bucket',
                                    PaginationConfig={'MaxItems': 10})

def unknown_client(ec2_client):
    # the argument could be an EC2 client like. But this is known only 
    # to the caller
    # ec2_client = boto3.client('ec2')

    # Get the paginator for the 'describe_instances' operation
    paginator = ec2_client.get_paginator('describe_instances')

    # Define any filters you want to apply (optional)
    # For example, to filter by running instances:
    filters = [
        {
            'Name': 'instance-state-name',
            'Values': ['running']
        }
    ]

    # Use the paginator to iterate through pages of results
    # You can also pass additional parameters to paginate(), such as MaxResults
    page_iterator = paginator.paginate(Filters=filters)

    # Iterate through each page and then through each reservation and instance
    for page in page_iterator:
        for reservation in page['Reservations']:
            for instance in reservation['Instances']:
                print(f"Instance ID: {instance['InstanceId']}")
                print(f"Instance Type: {instance['InstanceType']}")
                print(f"Launch Time: {instance['LaunchTime']}")
                print(f"State: {instance['State']['Name']}")
                print("-" * 20)