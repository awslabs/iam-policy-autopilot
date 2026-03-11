import software.amazon.awssdk.services.s3control.S3ControlClient;

class FileB {
    void run(S3ControlClient s3control) {
        s3control.putObject(request);
    }
}
