// TypeScript with type annotations and interfaces
import { DynamoDBClient, QueryCommand } from '@aws-sdk/client-dynamodb';
import { paginateQuery, QueryCommandInput } from '@aws-sdk/lib-dynamodb';
import { S3Client, GetObjectCommand, PutObjectCommand } from '@aws-sdk/client-s3';

// TypeScript interfaces and types
interface User {
  id: string;
  name: string;
  email: string;
}

interface S3Object {
  bucket: string;
  key: string;
  data?: any;
}

type QueryParams = QueryCommandInput & {
  TableName: string;
};

type S3Config = {
  region: string;
  credentials?: any;
};

// DynamoDB operations with TypeScript
const dynamoClient: DynamoDBClient = new DynamoDBClient({ region: 'us-east-1' });

async function queryUsers(): Promise<User[]> {
  const params: QueryParams = {
    TableName: 'Users',
    KeyConditionExpression: 'pk = :pk',
    ExpressionAttributeValues: {
      ':pk': 'USER'
    }
  };

  const command = new QueryCommand(params);
  const result = await dynamoClient.send(command);
  return result.Items as User[];
}

// S3 operations with TypeScript generics
class S3Service<T> {
  private client: S3Client;

  constructor(config: S3Config) {
    this.client = new S3Client(config);
  }

  async getObject(bucket: string, key: string): Promise<T> {
    const command = new GetObjectCommand({
      Bucket: bucket,
      Key: key
    });
    
    const result = await this.client.send(command);
    const bodyString = await result.Body?.transformToString() || '{}';
    return JSON.parse(bodyString) as T;
  }

  async putObject(bucket: string, key: string, data: T): Promise<void> {
    const command = new PutObjectCommand({
      Bucket: bucket,
      Key: key,
      Body: JSON.stringify(data),
      ContentType: 'application/json'
    });
    
    await this.client.send(command);
  }

  async processMultipleObjects(objects: S3Object[]): Promise<T[]> {
    const results: T[] = [];
    
    for (const obj of objects) {
      const data = await this.getObject(obj.bucket, obj.key);
      results.push(data);
    }
    
    return results;
  }
}

// Usage with proper TypeScript typing
const s3Service = new S3Service<User>({ region: 'us-west-2' });

async function processUserData(): Promise<void> {
  const users = await queryUsers();
  const userObjects: S3Object[] = users.map(user => ({
    bucket: 'user-data-bucket',
    key: `users/${user.id}.json`
  }));

  const processedUsers = await s3Service.processMultipleObjects(userObjects);
  console.log('Processed users:', processedUsers);
}

// Enum and advanced TypeScript features
enum ProcessingStatus {
  Pending = 'PENDING',
  Processing = 'PROCESSING',
  Completed = 'COMPLETED',
  Failed = 'FAILED'
}

interface ProcessingResult<T> {
  status: ProcessingStatus;
  data?: T;
  error?: string;
}

class DataProcessor<T> implements AsyncIterable<ProcessingResult<T>> {
  private items: T[] = [];

  public addItem(item: T): void {
    this.items.push(item);
  }

  async *[Symbol.asyncIterator](): AsyncIterator<ProcessingResult<T>> {
    for (const item of this.items) {
      try {
        // Simulate processing
        await new Promise(resolve => setTimeout(resolve, 100));
        yield {
          status: ProcessingStatus.Completed,
          data: item
        };
      } catch (error) {
        yield {
          status: ProcessingStatus.Failed,
          error: error instanceof Error ? error.message : 'Unknown error'
        };
      }
    }
  }
}

// Using the processor with TypeScript features
const processor = new DataProcessor<User>();
processor.addItem({ id: '1', name: 'Alice', email: 'alice@example.com' });
processor.addItem({ id: '2', name: 'Bob', email: 'bob@example.com' });

async function processAllUsers(): Promise<void> {
  for await (const result of processor) {
    console.log(`Processing result:`, result);
  }
}

// Execute main functions
processUserData().catch(console.error);
processAllUsers().catch(console.error);
