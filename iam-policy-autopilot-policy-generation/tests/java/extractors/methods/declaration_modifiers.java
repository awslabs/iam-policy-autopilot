// Tests that try-with-resources and formal parameter declarations with modifiers
// (final, annotations, and their combinations) are correctly resolved by the scope walk.
//
// Each method uses a distinct client type so the ReceiverDeclaration.TypeName is
// unambiguous in the expected output.
class DeclarationModifiers {

    // try-with-resources: final <Type> client = ...
    void finalResource() {
        try (final CloudVaultClient client = CloudVaultClient.create()) {
            client.describeResource(request);
        }
    }

    // try-with-resources: @Annotation <Type> client = ...
    void annotatedResource() {
        try (@SuppressWarnings("unused") DataLakeClient client = DataLakeClient.create()) {
            client.listObjects(request);
        }
    }

    // formal parameter: final <Type> param
    void finalFormalParam(final EventBridgeClient client) {
        client.putEvents(request);
    }

    // formal parameter: @Annotation <Type> param
    void annotatedFormalParam(@SuppressWarnings("unused") SecretManagerClient client) {
        client.getSecret(request);
    }
}
