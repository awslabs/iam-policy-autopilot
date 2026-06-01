class Test {
    void run() {
        S3Client s3a = S3Client.create(), s3b = S3Client.create();
        s3a.putObject(request);
        s3b.listBuckets();
    }
}
