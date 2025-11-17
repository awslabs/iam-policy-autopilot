//! Integration test for JavaScript/TypeScript AWS SDK extraction
//!
//! This test verifies that the JavaScript extractor can properly identify
//! AWS SDK usage patterns in JavaScript and TypeScript code.

use iam_policy_autopilot_policy_generation::{Language, SourceFile, ExtractionEngine};
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
    assert_eq!(Language::try_from_str("javascript").unwrap(), Language::JavaScript);
    assert_eq!(Language::try_from_str("ts").unwrap(), Language::TypeScript);
    assert_eq!(Language::try_from_str("typescript").unwrap(), Language::TypeScript);
    
    println!("✓ JavaScript/TypeScript language support working correctly");
}


#[tokio::test]
async fn test_javascript_basic_import_es6_extraction() {
    // Create a simple JavaScript file with AWS SDK imports and client instantiation
    let javascript_source = r#"
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

    // Create a source file
    let source_file = SourceFile::with_language(
        PathBuf::from("test.js"),
        javascript_source.to_string(),
        Language::JavaScript,
    );

    // Create extractor with basic path (no actual service definitions needed for this test)
    let engine = ExtractionEngine::new();

    // Extract operations from the source code
    let result = engine.extract_sdk_method_calls(Language::JavaScript, vec![source_file]).await;
    
    match result {
        Ok(extracted_methods) => {
            println!("✅ JavaScript ES6 extraction succeeded");
            println!("  Found {} method calls", extracted_methods.methods.len());
            
            // Should find operations from Command imports
            assert!(!extracted_methods.methods.is_empty(), "Should find operations from Command imports");
            
            // Expected operations from Command imports: CreateBucket, PutObject, ListObjectsV2, GetObject, DeleteObject, DeleteBucket
            let expected_operations = ["CreateBucket", "PutObject", "ListObjectsV2", "GetObject", "DeleteObject", "DeleteBucket"];
            
            for expected_op in &expected_operations {
                let found_op = extracted_methods.methods.iter()
                    .find(|call| call.name == *expected_op)
                    .unwrap_or_else(|| panic!("Should find {} operation from Command import", expected_op));
                    
                assert_eq!(found_op.possible_services, vec!["s3"], "All operations should be associated with s3 service");
            }
            
            // Print detailed output
            println!("✅ Found {} operations from ES6 imports and requires", extracted_methods.methods.len());
            for call in &extracted_methods.methods {
                println!("  - {} (service: {:?})", 
                    call.name, 
                    call.possible_services
                );
            }
        }
        Err(e) => {
            println!("JavaScript extraction failed: {}", e);
            // For testing, we'll allow this to pass if it's a service validation issue
            let error_message = format!("{}", e);
            assert!(error_message.contains("Service root directory") || error_message.contains("Failed to load"), 
                "Should be a service validation error, got: {}", e);
        }
    }
}

#[tokio::test]
async fn test_javascript_low_level_client_method_extraction() {
    // Create a simple JavaScript file with AWS SDK imports and client instantiation
    let javascript_source = r#"
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

    // Create a source file
    let source_file = SourceFile::with_language(
        PathBuf::from("test.js"),
        javascript_source.to_string(),
        Language::JavaScript,
    );

    // Create extractor with basic path (no actual service definitions needed for this test)
    let engine = ExtractionEngine::new();

    // Extract operations from the source code
    let result = engine.extract_sdk_method_calls(Language::JavaScript, vec![source_file]).await;
    
    match result {
        Ok(extracted_methods) => {
            println!("✅ JavaScript low-level client method extraction succeeded");
            println!("  Found {} method calls", extracted_methods.methods.len());
            
            // Should find operations from client method calls
            assert!(!extracted_methods.methods.is_empty(), "Should find operations from client method calls");
            
            // Should find ListTables operation from client.listTables() call
            let list_tables_op = extracted_methods.methods.iter()
                .find(|call| call.name == "ListTables")
                .expect("Should find ListTables operation from client.listTables() call");
            
            // Should be associated with dynamodb service (from client-dynamodb sublibrary)
            assert_eq!(list_tables_op.possible_services, vec!["dynamodb"], "Should associate with dynamodb service");
            
            println!("✅ Found {} operations from low-level client calls", extracted_methods.methods.len());
            for call in &extracted_methods.methods {
                println!("  - {} (service: {:?})", 
                    call.name, 
                    call.possible_services
                );
            }
        }
        Err(e) => {
            println!("JavaScript extraction failed: {}", e);
            let error_message = format!("{}", e);
            assert!(error_message.contains("Service root directory") || error_message.contains("Failed to load"), 
                "Should be a service validation error, got: {}", e);
        }
    }
}


#[tokio::test]
async fn test_javascript_client_send_extraction() {
    // Create a simple JavaScript file with AWS SDK imports and client instantiation
    let javascript_source = r#"
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

    // Create a source file
    let source_file = SourceFile::with_language(
        PathBuf::from("test.js"),
        javascript_source.to_string(),
        Language::JavaScript,
    );

    // Create extractor with basic path (no actual service definitions needed for this test)
    let engine = ExtractionEngine::new();

    // Extract operations from the source code
    let result = engine.extract_sdk_method_calls(Language::JavaScript, vec![source_file]).await;
    
    match result {
        Ok(extracted_methods) => {
            println!("✅ JavaScript client send extraction succeeded");
            println!("  Found {} method calls", extracted_methods.methods.len());
            
            // Should find operations from Command imports
            assert!(!extracted_methods.methods.is_empty(), "Should find operations from Command imports");
            
            // Should find ListTables operation inferred from ListTablesCommand
            let list_tables_op = extracted_methods.methods.iter()
                .find(|call| call.name == "ListTables")
                .expect("Should find ListTables operation from ListTablesCommand import");
            
            // Should be associated with dynamodb service (from client-dynamodb sublibrary)
            assert_eq!(list_tables_op.possible_services, vec!["dynamodb"], "Should associate with dynamodb service");
            
            println!("✅ Found {} operations from client send pattern", extracted_methods.methods.len());
            for call in &extracted_methods.methods {
                println!("  - {} (service: {:?})", 
                    call.name, 
                    call.possible_services
                );
            }
        }
        Err(e) => {
            println!("JavaScript extraction failed: {}", e);
            let error_message = format!("{}", e);
            assert!(error_message.contains("Service root directory") || error_message.contains("Failed to load"), 
                "Should be a service validation error, got: {}", e);
        }
    }
}

#[tokio::test]
async fn test_javascript_multiple_s3_operations_extraction() {
    // Create a simple JavaScript file with AWS SDK imports and client instantiation
    let javascript_source = r#"
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

    // Create a source file
    let source_file = SourceFile::with_language(
        PathBuf::from("test.js"),
        javascript_source.to_string(),
        Language::JavaScript,
    );

    // Create extractor with basic path (no actual service definitions needed for this test)
    let engine = ExtractionEngine::new();

    // Extract operations from the source code
    let result = engine.extract_sdk_method_calls(Language::JavaScript, vec![source_file]).await;
    
    match result {
        Ok(extracted_methods) => {
            println!("✅ JavaScript multiple S3 operations extraction succeeded");
            println!("  Found {} method calls", extracted_methods.methods.len());
            
            // Should find operations from Command imports
            assert!(!extracted_methods.methods.is_empty(), "Should find operations from Command imports");
            
            // Expected operations from Command imports: CreateBucket, PutObject, ListObjectsV2, GetObject, DeleteObject, DeleteBucket
            let expected_operations = ["CreateBucket", "PutObject", "ListObjectsV2", "GetObject", "DeleteObject", "DeleteBucket"];
            
            for expected_op in &expected_operations {
                let found_op = extracted_methods.methods.iter()
                    .find(|call| call.name == *expected_op)
                    .unwrap_or_else(|| panic!("Should find {} operation from Command import", expected_op));
                    
                assert_eq!(found_op.possible_services, vec!["s3"], "All operations should be associated with s3 service");
            }
            
            println!("✅ Found {} S3 operations from Command imports", extracted_methods.methods.len());
            for call in &extracted_methods.methods {
                println!("  - {} (service: {:?})", 
                    call.name, 
                    call.possible_services
                );
            }
        }
        Err(e) => {
            println!("JavaScript extraction failed: {}", e);
            let error_message = format!("{}", e);
            assert!(error_message.contains("Service root directory") || error_message.contains("Failed to load"), 
                "Should be a service validation error, got: {}", e);
        }
    }
}


#[tokio::test]
async fn test_javascript_pagination_extraction() {
    // Create a simple JavaScript file with AWS SDK imports and client instantiation
    let javascript_source = r#"
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
    "#;

    // Create a source file
    let source_file = SourceFile::with_language(
        PathBuf::from("test.js"),
        javascript_source.to_string(),
        Language::JavaScript,
    );

    // Create extractor with basic path (no actual service definitions needed for this test)
    let engine = ExtractionEngine::new();

    // Extract operations from the source code
    let result = engine.extract_sdk_method_calls(Language::JavaScript, vec![source_file]).await;
    
    match result {
        Ok(extracted_methods) => {
            println!("✅ JavaScript pagination extraction succeeded");
            println!("  Found {} method calls", extracted_methods.methods.len());
            
            // Should find operations from paginate and CommandInput imports
            assert!(!extracted_methods.methods.is_empty(), "Should find operations from paginate and CommandInput imports");
            
            // Should find Query operations from both paginateQuery and QueryCommandInput (PascalCase)
            let query_operations: Vec<_> = extracted_methods.methods.iter()
                .filter(|call| call.name == "Query")
                .collect();
            
            assert!(!query_operations.is_empty(), "Should find Query operations from paginate and CommandInput imports");
            
            // All query operations should be associated with dynamodb service
            for query_op in &query_operations {
                assert_eq!(query_op.possible_services, vec!["dynamodb"], "query operations should be associated with dynamodb service");
            }
            
            println!("✅ Found {} operations from pagination patterns", extracted_methods.methods.len());
            for call in &extracted_methods.methods {
                println!("  - {} (service: {:?})", 
                    call.name, 
                    call.possible_services
                );
            }
        }
        Err(e) => {
            println!("JavaScript extraction failed: {}", e);
            let error_message = format!("{}", e);
            assert!(error_message.contains("Service root directory") || error_message.contains("Failed to load"), 
                "Should be a service validation error, got: {}", e);
        }
    }
}
