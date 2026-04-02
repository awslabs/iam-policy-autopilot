import software.amazon.awssdk.services.s3.waiters.S3Waiter;
class Test {
    void run() {
        waiter.waitUntilBucketExists(request);
    }
}
