# syntax = docker/dockerfile:1.7

FROM --platform=linux/amd64 docker-cja-arrow-dev.dr-uw2.adobeitc.com/cargo-chef:0.1.66-rust-slim-bookworm AS chef
WORKDIR /app

FROM --platform=linux/amd64 chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM --platform=linux/amd64 chef AS builder 
COPY --from=planner /app/recipe.json recipe.json
RUN <<eof
#!/bin/bash
cd /var/tmp
mkdir proto
cd proto
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install --assume-yes wget unzip
wget https://github.com/protocolbuffers/protobuf/releases/download/v26.1/protoc-26.1-linux-x86_64.zip
unzip protoc-26.1-linux-x86_64.zip
cp -a ./bin/* /usr/bin/
cp -a ./include/* /usr/include/
eof
RUN cargo chef cook --recipe-path recipe.json
COPY . .
RUN cargo build --bin ballista-scheduler --bin ballista-executor --bin ballista-cli

FROM --platform=linux/amd64 debian:bookworm-slim AS runtime
RUN <<eof
  #!/bin/bash
  apt-get update && apt-get -y install curl psutils less awscli python3-pip
eof

COPY --from=builder /app/target/debug/ballista-scheduler /usr/local/bin/ballista-scheduler
COPY --from=builder /app/target/debug/ballista-executor /usr/local/bin/ballista-executor
COPY --from=builder /app/target/debug/ballista-cli /usr/local/bin/ballista-cli

WORKDIR /app

