// Wildcard import for the enhanced.dynamodb package with an explicit DynamoDbTable parameter type.
// The import extractor currently discards wildcard utility imports, so utility_imports_by_file
// is empty and the call is not matched. This fixture demonstrates the gap.
import software.amazon.awssdk.enhanced.dynamodb.*;

class Test {
    void run(DynamoDbTable<Customer> table) {
        table.putItem(item);
    }
}
