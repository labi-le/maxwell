FROM rust:1.98.1-bookworm
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg git \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && touch src/main.rs && cargo fetch --locked && chmod -R a+rwX /usr/local/cargo
