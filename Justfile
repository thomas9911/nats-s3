run:
    RUST_LOG=debug cargo run -- --admin-access-key YOUR_ACCESS_KEY --admin-secret-key YOUR_SECRET_KEY --nats-address localhost:4222

seed:
    nats kv add s3-auth-localhost_9514
    nats kv put s3-auth-localhost_9514 youraccesskey yoursecretkey