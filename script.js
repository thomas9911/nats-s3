import bun from "bun";
import { S3Client, ListBucketsCommand, CreateBucketCommand } from "@aws-sdk/client-s3";

const credentials = {
	accessKeyId: "youraccesskey",
	secretAccessKey: "yoursecretkey",
    endpoint: "http://localhost:9514",
}

// const client = new S3Client({
// 	bucket: "protected",
//     ...credentials
// });

// console.log(await client.list());

const client = new S3Client({
	region: "us-east-1",
	  credentials: {
       accessKeyId: credentials.accessKeyId,
       secretAccessKey: credentials.secretAccessKey,
   },
	endpoint: credentials.endpoint,
	forcePathStyle: true
 });

 
const command = new ListBucketsCommand({});
// const command = new CreateBucketCommand({
// 	Bucket: "my-new-bucket"
// });

const data = await client.send(command);
// console.log(data);

const bunClient = bun.S3Client(
	{
		bucket: "my-new-bucket",
		...credentials
	}
)

const file = bunClient.file("test.txt");

console.log(await file.text());
