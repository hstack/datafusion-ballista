# syntax = docker/dockerfile:1.7

FROM rust:1.78.0-bookworm as builder
ARG TARGETARCH
ARG TARGETPLATFORM

WORKDIR /app

RUN <<eof
#!/bin/bash
  apt-get update
  DEBIAN_FRONTEND=noninteractive apt-get install --assume-yes wget unzip yarnpkg
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

COPY . .
# on the rust image, the CARGO_HOME is set to /usr/local/cargo
RUN --mount=type=cache,target=/usr/local/cargo,from=rust,source=/usr/local/cargo \
    --mount=type=cache,target=/app/target \
<<eof
#!/bin/bash
    cargo build --bin ballista-scheduler --bin ballista-executor --bin ballista-cli
    # we need to copy the files out of cache
    # there is no way to get to them after this RUN statement
    mkdir -p /usr/local/bin
    cp /app/target/debug/ballista-scheduler /usr/local/bin/
    cp /app/target/debug/ballista-executor /usr/local/bin/
    cp /app/target/debug/ballista-cli /usr/local/bin/
eof

RUN --mount=type=cache,target=/root/.yarn \
<<eof
#!/bin/bash
  export YARN_CACHE_FOLDER=/root/.yarn
  pushd /app/ballista/scheduler/ui
  yarnpkg install
  yarnpkg build
eof

FROM debian:bookworm-slim AS runtime
RUN <<eof
  #!/bin/bash
  apt-get update && apt-get -y install curl psutils less awscli python3-pip nginx netcat-traditional
eof

COPY --from=builder /usr/local/bin/ballista-scheduler /usr/local/bin/ballista-scheduler
COPY --from=builder /usr/local/bin/ballista-executor /usr/local/bin/ballista-executor
COPY --from=builder /usr/local/bin/ballista-cli /usr/local/bin/ballista-cli
COPY --from=builder /app/ballista/scheduler/ui/build /var/www/html
COPY --from=builder /app/dev/docker/nginx.conf /etc/nginx/sites-enabled/default

WORKDIR /app


