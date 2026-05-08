FROM rust:1.85-slim AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release || true

COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app
COPY --from=builder /app/target/release/shellshock-api /usr/local/bin/shellshock-api
ENV PORT=8080
EXPOSE 8080
CMD ["shellshock-api"]
