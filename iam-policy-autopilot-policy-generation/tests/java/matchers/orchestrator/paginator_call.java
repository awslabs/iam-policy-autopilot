class Test {
    void run() {
        ListObjectsV2Iterable pages = s3Client.listObjectsV2Paginator(request);
    }
}
