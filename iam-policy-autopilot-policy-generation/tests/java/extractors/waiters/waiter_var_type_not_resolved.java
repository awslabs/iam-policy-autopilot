class WaiterVarTypeNotResolved {
    void run(Object cloudVaultClient) {
        var waiter = cloudVaultClient.waiter();
        waiter.waitUntilResourceReady(request);
    }
}
