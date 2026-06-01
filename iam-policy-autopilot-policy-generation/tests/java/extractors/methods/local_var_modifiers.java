// Tests that local variable declarations with modifiers (final, annotations, and their
// combinations) are correctly resolved by the scope walk.
//
// Each method uses a distinct client type so the ReceiverDeclaration.TypeName is
// unambiguous in the expected output.
class LocalVarModifiers {

    // final <Type> client = ...
    void finalTyped() {
        final CloudVaultClient client = CloudVaultClient.create();
        client.describeResource(request);
    }

    // @Annotation <Type> client = ...
    void annotatedTyped() {
        @SuppressWarnings("unused") DataLakeClient client = DataLakeClient.create();
        client.listObjects(request);
    }

    // @Annotation final <Type> client = ...
    void annotatedFinalTyped() {
        @SuppressWarnings("unused") final EventBridgeClient client = EventBridgeClient.create();
        client.putEvents(request);
    }

    // final @Annotation <Type> client = ...
    void finalAnnotatedTyped() {
        final @SuppressWarnings("unused") SecretManagerClient client = SecretManagerClient.create();
        client.getSecret(request);
    }

    // final var client = ...  (type_name is null because var is inferred)
    void finalVar() {
        final var client = KeyVaultClient.create();
        client.getKey(request);
    }

    // @Annotation var client = ...  (type_name is null because var is inferred)
    void annotatedVar() {
        @SuppressWarnings("unused") var client = QueueServiceClient.create();
        client.sendMessage(request);
    }
}
