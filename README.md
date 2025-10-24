# Promote Me!
A discord bot that promotes users who go online on Twitch, new posts on Bluesky and whenever a new post is made on an RSS feed.


## Build
Build for you target.
```bash
cargo build --release --target x86_64-unknown-linux-gnu
```

## Deployment
Example of deploying the entire stack to AWS.
```bash
sudo chmod +x deploy.sh
BINARY=target/release/promote-me \
BUCKET=promote-me-discord-bot \
TEMPLATE=template.yaml \
STACK_NAME=promote-me-stack \
REGION=eu-west-1 \
PARAM_OVERRIDES=KeyPairName=promote-me-key \
./deploy.sh
```