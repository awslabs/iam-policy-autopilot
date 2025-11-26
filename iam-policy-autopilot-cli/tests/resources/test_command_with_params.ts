import { S3Client, CreateBucketCommand, PutObjectCommand } from "@aws-sdk/client-s3";

const client = new S3Client({ region: "us-east-1" });

// Test with literal bucket name
const command1 = new CreateBucketCommand({ Bucket: "my-test-bucket" });

// Test with variable bucket name
const bucketName = "another-bucket";
const command2 = new PutObjectCommand({ 
  Bucket: bucketName,
  Key: "file.txt",
  Body: "content"
});

await client.send(command1);
await client.send(command2);
