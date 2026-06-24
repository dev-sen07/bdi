FROM rust:latest AS builder

WORKDIR /usr/src/app/services/rust-engine

# Instalar protobuf compiler
RUN apt-get update && apt-get install -y protobuf-compiler

COPY services/rust-engine/Cargo.toml services/rust-engine/build.rs ./
COPY services/rust-engine/src ./src

# Replicamos la estructura del host para que ../../proto sea /usr/src/app/proto
COPY proto /usr/src/app/proto

# Compilar proyecto en release
RUN cargo build --release

# Etapa runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y libssl3 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/app/services/rust-engine/target/release/rust-engine /usr/local/bin/rust-engine

EXPOSE 50052

CMD ["rust-engine"]
