// import { S3Client, CreateBucketCommand, waitUntilBucketExists } from "@aws-sdk/client-s3";
import { S3Client, waitUntilBucketExists } from "@aws-sdk/client-s3";

// Configure your S3 client
const client = new S3Client({ region: "us-east-1" }); // Replace with your desired region

// Define your bucket name
const bucketName = "my-unique-test-bucket-12345"; 

async function createAndAwaitBucket() {
  try {
    // 1. Create the S3 bucket
    // const createBucketCommand = new CreateBucketCommand({ Bucket: bucketName });
    // await client.send(createBucketCommand);
    console.log(`Bucket "${bucketName}" creation initiated.`);

    // 2. Wait for the bucket to exist
    // waitUntilBucketExists polls the S3 service until the bucket exists or maxWaitTime is reached.
    await waitUntilBucketExists(
      { client, maxWaitTime: 60, minDelay: 1, maxDelay: 10 }, // Waiter configuration
      { Bucket: bucketName } // Input for the waiter's underlying API call
    );
    console.log(`Bucket "${bucketName}" now exists.`);

  } catch (error) {
    console.error("Error:", error);
  }
}

createAndAwaitBucket();