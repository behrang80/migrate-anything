# syntax=docker/dockerfile:1
# Multi-stage build: compile in the official Rust image, run as a non-root user in a slim image.
FROM rust:1.100-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p migrate-anything

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
 && useradd --create-home --uid 10001 migrate
COPY --from=build /src/target/release/migrate-anything /usr/local/bin/migrate-anything
USER migrate
ENV MA_HOME=/home/migrate/.migrate-anything
VOLUME ["/home/migrate/.migrate-anything", "/data"]
WORKDIR /data
ENTRYPOINT ["migrate-anything"]
CMD ["doctor"]
