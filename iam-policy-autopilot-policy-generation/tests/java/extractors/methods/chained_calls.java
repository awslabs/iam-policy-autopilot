class Test {
    void run() {
        s3Client.putObject(PutObjectRequest.builder().bucket("my-bucket").key("my-key").build());
    }
}
