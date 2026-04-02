// No import — receiver type is fully-qualified in the variable declaration.
class Test {
    void run(software.amazon.awssdk.transfer.s3.S3TransferManager transferManager) {
        transferManager.uploadFile(req);
    }
}
