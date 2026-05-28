import software.amazon.awssdk.services.sqs.batchmanager.SqsAsyncBatchManager;

class Test {
    void run(SqsAsyncBatchManager batchManager) {
        batchManager.deleteMessage(req);
    }
}
