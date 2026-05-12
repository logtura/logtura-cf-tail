# syntax=docker/dockerfile:1.7
#
# Static musl build in a scratch image. Consumers can copy just the
# binary into a Vector forwarder image:
#   COPY --from=ghcr.io/logtura/logtura-cf-tail:vX.Y.Z \
#        /logtura-cf-tail /usr/local/bin/logtura-cf-tail

FROM rust:1.95-slim AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends \
      musl-tools \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY tests ./tests
RUN cargo build --release --target x86_64-unknown-linux-musl --bin logtura-cf-tail

FROM scratch
COPY --from=build /src/target/x86_64-unknown-linux-musl/release/logtura-cf-tail /logtura-cf-tail
ENTRYPOINT ["/logtura-cf-tail"]
