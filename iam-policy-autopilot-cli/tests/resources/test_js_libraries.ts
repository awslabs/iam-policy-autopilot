import { DynamoDBDocumentClient, PutCommand, GetCommand, ScanCommand } from '@aws-sdk/lib-dynamodb';

import { S3Client } from "@aws-sdk/client-s3";
import { Upload } from "@aws-sdk/lib-storage";
import * as fs from "fs";

const s3Client = new S3Client({ region: "us-west-2" });

async function uploadLargeFile() {
  const fileStream = fs.createReadStream("large-file.dat");
  
  const upload = new Upload({
    client: s3Client,
    params: {
      Bucket: "my-bucket",
      Key: "uploads/large-file.dat",
      Body: fileStream,
    },
    tags: [
      { Key: "Environment", Value: "Production" },
      { Key: "Project", Value: "DataPipeline" }
    ],
    queueSize: 4,
    partSize: 1024 * 1024 * 5, // 5MB parts
  });

  upload.on("httpUploadProgress", (progress) => {
    console.log(`Uploaded ${progress.loaded} of ${progress.total} bytes`);
  });

  try {
    const result = await upload.done();
    console.log("Upload completed:", result.Location);
  } catch (error) {
    console.error("Upload failed:", error);
  }
}

uploadLargeFile();
