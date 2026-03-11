import software.amazon.awssdk.services.cloudvault.waiters.CloudVaultWaiter;

class WaiterCallParameterTypeResolved {
    void run(CloudVaultWaiter waiter) {
        waiter.waitUntilResourceReady(request);
    }
}
