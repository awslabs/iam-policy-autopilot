import software.amazon.awssdk.services.cloudvault.waiters.CloudVaultWaiter;

class WaiterCallReceiverAndTypeCaptured {
    void run(Object cloudVaultClient) {
        CloudVaultWaiter waiter = cloudVaultClient.waiter();
        waiter.waitUntilResourceReady(request);
    }
}
