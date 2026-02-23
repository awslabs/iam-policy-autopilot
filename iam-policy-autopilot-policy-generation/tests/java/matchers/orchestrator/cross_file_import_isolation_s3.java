import software.amazon.awssdk.services.s3.S3Client;

class FileA {
    void run(S3Client s3) {
        s3.putObject(request);
    }
}
