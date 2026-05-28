class Test {
    void run() {
        s3Client.listBuckets();
        s3Client.putObject(request);
        dynamoClient.getItem(getItemRequest);
    }
}
