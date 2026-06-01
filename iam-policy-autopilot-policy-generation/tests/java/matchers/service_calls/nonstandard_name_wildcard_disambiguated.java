// Wildcard imports for two services that share the same method.
// The disambiguator must derive "DynamoDbClient" from serviceId "DynamoDB" via
// java_service_name("DynamoDB") = "DynamoDb", then append "Client".
// The competing service "DynamoDB V2" → "DynamoDbV2" → "DynamoDbV2Client" does NOT match,
// so the call is pinned to dynamodb.
import software.amazon.awssdk.services.dynamodb.*;
import software.amazon.awssdk.services.dynamodbv2.*;

class NonstandardNameWildcardDisambiguated {
    void run(DynamoDbClient client) {
        client.describeTable(request);
    }
}
