class Test {
    void run() {
        for (ListObjectsV2Response r : s3Client.listObjectsV2Paginator(request)) {
        }
    }
}
