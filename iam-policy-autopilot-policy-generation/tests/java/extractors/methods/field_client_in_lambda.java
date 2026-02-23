import software.amazon.awssdk.services.s3.S3AsyncClient;

class FieldClientInLambda {
    private final S3AsyncClient s3 = S3AsyncClient.create();

    void run() {
        s3.listBuckets()
            .thenCompose(resp -> s3.headBucket(r -> r.bucket("my-bucket")));
    }
}
