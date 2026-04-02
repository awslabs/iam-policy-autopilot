// No import statement — receiver type is fully-qualified in the variable declaration.
// The disambiguator must extract the service directly from the FQN prefix.
class WaiterFqnTypeDisambiguated {
    void run() {
        software.amazon.awssdk.services.cloudvault.waiters.CloudVaultWaiter waiter =
                cloudVaultClient.waiter();
        waiter.waitUntilResourceReady(request);
    }
}
