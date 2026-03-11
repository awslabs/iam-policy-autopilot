// No import statement — receiver type is fully-qualified in the variable declaration.
// The disambiguator must extract the service directly from the FQN prefix.
class ServiceCallFqnTypeDisambiguated {
    void run() {
        software.amazon.awssdk.services.cloudvault.CloudVaultClient client =
                CloudVaultClient.create();
        client.describeResource(request);
    }
}
