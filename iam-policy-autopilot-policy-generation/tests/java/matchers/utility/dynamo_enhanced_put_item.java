import software.amazon.awssdk.enhanced.dynamodb.DynamoDbTable;

class Test {
    void run(DynamoDbTable<Customer> table) {
        table.putItem(item);
    }
}
