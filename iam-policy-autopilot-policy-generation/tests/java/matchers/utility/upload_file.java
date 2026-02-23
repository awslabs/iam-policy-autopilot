import software.amazon.awssdk.transfer.s3.S3TransferManager;

class Test {
    void run(S3TransferManager transferManager) {
        transferManager.uploadFile(req);
    }
}
