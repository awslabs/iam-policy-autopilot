//! Integration test for JavaScript/TypeScript AWS SDK extraction
//!
//! This test verifies that the JavaScript extractor can properly identify
//! AWS SDK usage patterns in JavaScript and TypeScript code.

use iam_policy_autopilot_policy_generation::{ExtractionEngine, Language, SourceFile};
use rstest::rstest;
use std::path::PathBuf;

#[test]
fn test_javascript_language_support() {
    // Test that JavaScript/TypeScript languages are properly supported
    use iam_policy_autopilot_policy_generation::Language;

    // Test language enum values
    assert_eq!(Language::JavaScript.to_string(), "javascript");
    assert_eq!(Language::TypeScript.to_string(), "typescript");

    // Test language parsing
    assert_eq!(Language::try_from_str("js").unwrap(), Language::JavaScript);
    assert_eq!(
        Language::try_from_str("javascript").unwrap(),
        Language::JavaScript
    );
    assert_eq!(Language::try_from_str("ts").unwrap(), Language::TypeScript);
    assert_eq!(
        Language::try_from_str("typescript").unwrap(),
        Language::TypeScript
    );

    println!("✓ JavaScript/TypeScript language support working correctly");
}

// ---------------------------------------------------------------------------
// Expected operations per test case
// ---------------------------------------------------------------------------

struct ExpectedOp {
    name: &'static str,
    service: &'static str,
}

const S3_CRUD_OPS: &[ExpectedOp] = &[
    ExpectedOp {
        name: "CreateBucket",
        service: "s3",
    },
    ExpectedOp {
        name: "PutObject",
        service: "s3",
    },
    ExpectedOp {
        name: "ListObjectsV2",
        service: "s3",
    },
    ExpectedOp {
        name: "GetObject",
        service: "s3",
    },
    ExpectedOp {
        name: "DeleteObject",
        service: "s3",
    },
    ExpectedOp {
        name: "DeleteBucket",
        service: "s3",
    },
];

// ---------------------------------------------------------------------------
// Test sources
// ---------------------------------------------------------------------------

const ES6_IMPORT_SOURCE: &str = r#"
import {
  S3Client,
  CreateBucketCommand,
  PutObjectCommand as PutObject,
  ListObjectsV2Command,
  GetObjectCommand,
  DeleteObjectCommand,
  DeleteBucketCommand,
} from "@aws-sdk/client-s3";
import S3Client from "@aws-sdk/client-s3";


const { S3Client, CreateBucketCommand } = require("@aws-sdk/client-s3");
const s3ClientRequire = new S3Client({ region: "us-east-1" });
async function createMyBucket() {
  const command = new CreateBucketCommand({ Bucket: "my-unique-bucket-name" });
  try {
    const data = await s3ClientRequire.send(command);
    console.log("Bucket created:", data);
  } catch (error) {
    console.error("Error creating bucket:", error);
  }
}
createMyBucket();
"#;

const LOW_LEVEL_CLIENT_SOURCE: &str = r#"
const { DynamoDB } = require("@aws-sdk/client-dynamodb");

(async () => {
  const client = new DynamoDB({ region: "us-west-2" });
  try {
    const results = await client.listTables({});
    console.log(results.TableNames.join("\n"));
  } catch (err) {
    console.error(err);
  }
})();
"#;

const CLIENT_SEND_SOURCE: &str = r#"
const { DynamoDBClient, ListTablesCommand } = require("@aws-sdk/client-dynamodb");

(async () => {
  const client = new DynamoDBClient({ region: "us-west-2" });
  const command = new ListTablesCommand({});
  try {
    const results = await client.send(command);
    console.log(results.TableNames.join("\n"));
  } catch (err) {
    console.error(err);
  }
})();
"#;

const MULTIPLE_S3_OPS_SOURCE: &str = r#"
import {
  CreateBucketCommand,
  PutObjectCommand as PutObject,
  ListObjectsV2Command,
  GetObjectCommand,
  DeleteObjectCommand,
  DeleteBucketCommand,
} from "@aws-sdk/client-s3";
import S3Client from "@aws-sdk/client-s3";

// Define your bucket and object details
const bucketName = "my-unique-js-example-bucket";
const objectKey = "example-upload.txt";
const fileContent = "This is a test file content.";
const downloadedFile = "downloaded-file.txt";

// --- S3 Operations ---

const s3Client = new S3Client({
  region: "us-east-1",
});

// 1. Create a new S3 bucket
async function createS3Bucket() {
  try {
    console.log(`Creating bucket: ${bucketName}`);
    const createBucketParams = {
      Bucket: bucketName,
    };
    await s3Client.send(new CreateBucketCommand(createBucketParams));
    console.log(`Bucket '${bucketName}' created successfully.`);
  } catch (err) {
    console.error("Error creating bucket:", err);
    throw err;
  }
}

// 2. Upload an object to the bucket
async function uploadObject() {
  try {
    console.log(`Uploading object '${objectKey}' to bucket '${bucketName}'`);
    const putObjectParams = {
      Bucket: bucketName,
      Key: objectKey,
      Body: fileContent,
    };
    await s3Client.send(new PutObject(putObjectParams));
    console.log("Object uploaded successfully.");
  } catch (err) {
    console.error("Error uploading object:", err);
    throw err;
  }
}

// 3. List objects in the bucket
async function listObjects() {
  try {
    console.log(`Listing objects in bucket '${bucketName}'`);
    const listObjectsParams = {
      Bucket: bucketName,
    };
    const { Contents } = await s3Client.send(new ListObjectsV2Command(listObjectsParams));

    if (Contents && Contents.length > 0) {
      console.log("Objects found:");
      Contents.forEach(obj => console.log(` - ${obj.Key}`));
    } else {
      console.log("No objects found.");
    }
  } catch (err) {
    console.error("Error listing objects:", err);
    throw err;
  }
}

// 4. Download an object from the bucket
async function downloadObject() {
  try {
    console.log(`Downloading object '${objectKey}' from bucket '${bucketName}'`);
    const getObjectParams = {
      Bucket: bucketName,
      Key: objectKey,
    };
    const { Body } = await s3Client.send(new GetObjectCommand(getObjectParams));

    if (Body instanceof Readable) {
      const data = await new Promise((resolve, reject) => {
        let chunks = [];
        Body.on('data', chunk => chunks.push(chunk));
        Body.on('end', () => resolve(Buffer.concat(chunks).toString()));
        Body.on('error', reject);
      });
      console.log("Object content:");
      console.log(data);
      // Optional: Save to a local file
      // writeFileSync(downloadedFile, data);
      // console.log(`Content saved to '${downloadedFile}'.`);
    } else {
      console.error("Downloaded object body is not a readable stream.");
    }
  } catch (err) {
    console.error("Error downloading object:", err);
    throw err;
  }
}

// 5. Delete an object from the bucket
async function deleteObject() {
  try {
    console.log(`Deleting object '${objectKey}'`);
    const deleteObjectParams = {
      Bucket: bucketName,
      Key: objectKey,
    };
    await s3Client.send(new DeleteObjectCommand(deleteObjectParams));
    console.log("Object deleted successfully.");
  } catch (err) {
    console.error("Error deleting object:", err);
    throw err;
  }
}

// 6. Delete the S3 bucket
async function deleteS3Bucket() {
  try {
    console.log(`Deleting bucket: ${bucketName}`);
    const deleteBucketParams = {
      Bucket: bucketName,
    };
    await s3Client.send(new DeleteBucketCommand(deleteBucketParams));
    console.log(`Bucket '${bucketName}' deleted successfully.`);
  } catch (err) {
    console.error("Error deleting bucket:", err);
    throw err;
  }
}

// Run the sequence of operations
async function runS3Operations() {
  try {
    await createS3Bucket();
    await uploadObject();
    await listObjects();
    await downloadObject();
    await deleteObject();
    await deleteS3Bucket();
  } catch (err) {
    console.error("A critical error occurred during S3 operations.");
  }
}

runS3Operations();
"#;

const PAGINATION_SOURCE: &str = r"
import { DynamoDBClient } from '@aws-sdk/client-dynamodb';
import {
  DynamoDBDocument,
  QueryCommandInput,
  paginateQuery,
  DynamoDBDocumentPaginationConfiguration
} from '@aws-sdk/lib-dynamodb';

const TABLE_NAME2 = 'my-table';

function getRegion(){
  return 'eu-west-1'
}

const REGION = getRegion();
const TABLE_NAME = TABLE_NAME2;
const PK_QUERY_VALUE = 'my-pk';

// Create a DynamoDB Document client
const docClient = DynamoDBDocument.from(
  new DynamoDBClient({
    region: REGION
  })
);

// Create a paginator configuration
const paginatorConfig: DynamoDBDocumentPaginationConfiguration = {
  client: docClient,
  pageSize: 25
};

// Query parameters
const params: QueryCommandInput = {
  TableName: TABLE_NAME,
  KeyConditionExpression: 'pk = :pk',
  ExpressionAttributeValues: {
    ':pk': PK_QUERY_VALUE
  }
};

// Create a paginator
const paginator = paginateQuery(paginatorConfig, params);

// Paginate until there are no more results
const items: any[] = [];
for await (const page of paginator) {
  items.push(...page.Items);
}
";

// ---------------------------------------------------------------------------
// Parameterized extraction test
// ---------------------------------------------------------------------------

#[rstest]
#[case::es6_imports(ES6_IMPORT_SOURCE, S3_CRUD_OPS)]
#[case::low_level_client(LOW_LEVEL_CLIENT_SOURCE, &[ExpectedOp { name: "ListTables", service: "dynamodb" }])]
#[case::client_send(CLIENT_SEND_SOURCE, &[ExpectedOp { name: "ListTables", service: "dynamodb" }])]
#[case::multiple_s3_operations(MULTIPLE_S3_OPS_SOURCE, S3_CRUD_OPS)]
#[case::pagination(PAGINATION_SOURCE, &[ExpectedOp { name: "Query", service: "dynamodb" }])]
#[tokio::test]
async fn test_javascript_extraction(#[case] source: &str, #[case] expected_ops: &[ExpectedOp]) {
    let source_file = SourceFile::with_language(
        PathBuf::from("test.js"),
        source.to_string(),
        Language::JavaScript,
    );

    let engine = ExtractionEngine::new();
    let extracted_methods = engine
        .extract_sdk_method_calls(Language::JavaScript, vec![source_file])
        .await
        .expect("JavaScript extraction must succeed");

    assert!(
        !extracted_methods.methods.is_empty(),
        "Should extract at least one SDK method call"
    );

    for expected in expected_ops {
        let found = extracted_methods
            .methods
            .iter()
            .find(|call| call.name == expected.name)
            .unwrap_or_else(|| panic!("Expected operation '{}' not found", expected.name));

        assert_eq!(
            found.possible_services,
            vec![expected.service],
            "Operation '{}' should be associated with '{}' service",
            expected.name,
            expected.service
        );
    }
}
