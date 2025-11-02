import { S3Client } from "bun";

const client = new S3Client({
	accessKeyId: "youraccesskey",
	secretAccessKey: "yoursecretkey",
	bucket: "protected",
    endpoint: "http://localhost:9514",
});

console.log(await client.list());