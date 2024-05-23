# syntax = docker/dockerfile:1.7

FROM lukemathwalker/cargo-chef:0.1.66-rust-slim-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder 
ARG TARGETARCH
ARG TARGETPLATFORM
RUN <<eof
#!/bin/bash
  apt-get update
  DEBIAN_FRONTEND=noninteractive apt-get install --assume-yes wget unzip yarnpkg
eof

RUN <<eof
#!/bin/bash
  mkdir -p /var/tmp/proto
  pushd /var/tmp/proto
  if [[ "${TARGETARCH}" == "amd64" ]]; then
    protoc_file=protoc-26.1-linux-x86_64.zip
  else
    protoc_file=protoc-26.1-linux-aarch_64.zip
  fi
  wget https://github.com/protocolbuffers/protobuf/releases/download/v26.1/${protoc_file}
  unzip ${protoc_file}
  cp -a ./bin/* /usr/bin/
  cp -a ./include/* /usr/include/
eof

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --recipe-path recipe.json
COPY . .
RUN cargo build --bin ballista-scheduler --bin ballista-executor --bin ballista-cli

RUN <<eof
#!/bin/bash
  pushd /app/ballista/scheduler/ui
  yarnpkg install
  yarnpkg build
eof

FROM debian:bookworm-slim AS runtime
RUN <<eof
  #!/bin/bash
  apt-get update && apt-get -y install curl psutils less awscli python3-pip nginx netcat-traditional
eof

COPY --from=builder /app/target/debug/ballista-scheduler /usr/local/bin/ballista-scheduler
COPY --from=builder /app/target/debug/ballista-executor /usr/local/bin/ballista-executor
COPY --from=builder /app/target/debug/ballista-cli /usr/local/bin/ballista-cli
COPY --from=builder /app/ballista/scheduler/ui/build /var/www/html
COPY --from=builder /app/dev/docker/nginx.conf /etc/nginx/sites-enabled/default

WORKDIR /app

