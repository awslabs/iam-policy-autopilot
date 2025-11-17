// Import via require function, interaction via send()
let { S3Client, CreateBucketCommand } = require("@aws-sdk/client-s3");
const s3Client = new S3Client({ region: "us-east-1" });
async function createMyBucket() {
  const command = new CreateBucketCommand({ Bucket: "my-bucket-name" });
  try {
    const data = await s3Client.send(command);
    console.log("Bucket created:", data);
  } catch (error) {
    console.error("Error creating bucket:", error);
  }
}
createMyBucket();

// Import directive, interactions via send and paginator
import { DynamoDBClient as DBClientRenamed } from '@aws-sdk/client-dynamodb';
import {
  DynamoDBDocument,
  QueryCommandInput as QueryInputRenamed,
  ListBackupsInput,
  paginateQuery
} from '@aws-sdk/lib-dynamodb';

let TABLE_NAME2 = 'my-table';

const REGION = 'eu-west-1';
const TABLE_NAME = TABLE_NAME2;
const PK_QUERY_VALUE = 'my-pk';

// Create a DynamoDB Document client
const docClient = DynamoDBDocument.from(
  new DBClientRenamed({
    region: REGION
  })
);

// Create a paginator configuration (removed type annotation)
const paginatorConfig = {
  client: docClient,
  pageSize: 25
};

// Query parameters (removed type annotation)
const params = {
  TableName: TABLE_NAME,
  KeyConditionExpression: 'pk = :pk',
  ExpressionAttributeValues: {
    ':pk': PK_QUERY_VALUE
  }
};

// Create a paginator
const paginator = paginateQuery(paginatorConfig, params);

// Paginate until there are no more results (removed type annotation)
const items = [];
for await (const page of paginator) {
  items.push(...page.Items);
}

// Paginator
const { DynamoDBClient, paginateListTables } = require("@aws-sdk/client-dynamodb");

async function getAllDynamoDBTables() {
    const client = new DynamoDBClient({ region: "an-aws-region" });
    const paginatorConfig = { client };

    let allTableNames = [];

    try {
        for await (const page of paginateListTables(paginatorConfig, {})) {
            if (page.TableNames) {
                allTableNames = allTableNames.concat(page.TableNames);
            }
        }
        console.log("All DynamoDB Table Names:", allTableNames);
        return allTableNames;
    } catch (error) {
        console.error("Error listing tables:", error);
        throw error;
    }
}

getAllDynamoDBTables();

// stream pattern
import { S3 } from "@aws-sdk/client-s3";

const client = new S3({region: REGION});

const anotherBucket = "another-bucket"
const getObjectResult = await client.getObject({
  Bucket: anotherBucket,
  Key: "another-key",
});

// env-specific stream with added mixin methods.
const bodyStream = getObjectResult.Body;

// one-time transform.
const bodyAsString = await bodyStream.transformToString();

// throws an error on 2nd call, stream cannot be rewound.
const __error__ = await bodyStream.transformToString();
