import software.amazon.awssdk.services.s3.presigner.S3Presigner;

class Test {
    void run(S3Presigner presigner) {
        presigner.presignGetObject(req);
    }
}
