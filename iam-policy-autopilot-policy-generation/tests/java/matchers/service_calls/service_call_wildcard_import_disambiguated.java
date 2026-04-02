// Wildcard import only — no specific import for CloudVaultClient.
// The disambiguator must derive "CloudVaultClient" from serviceId "CloudVault" + "Client" suffix.
import software.amazon.awssdk.services.cloudvault.*;

class ServiceCallWildcardImportDisambiguated {
    void run(CloudVaultClient client) {
        client.describeResource(request);
    }
}
