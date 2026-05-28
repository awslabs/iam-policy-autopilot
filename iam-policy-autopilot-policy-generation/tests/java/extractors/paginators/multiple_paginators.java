class Test {
    void run() {
        ListObjectsV2Iterable pages = s3Client.listObjectsV2Paginator(req1);
        ScanIterable scans = dynamoClient.scanPaginator(req2);
    }
}
