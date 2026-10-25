FROM rust:1-slim-trixie AS build
RUN apt-get update && apt-get install -y --no-install-recommends gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release --no-default-features --bin our-clipboard

FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libgcc-s1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/target/release/our-clipboard /usr/local/bin/our-clipboard
USER 65532:65532
EXPOSE 8443
ENTRYPOINT ["/usr/local/bin/our-clipboard"]
CMD ["relay", "--listen", "0.0.0.0:8443", "--tls-cert", "/tls/fullchain.pem", "--tls-key", "/tls/privkey.pem"]
