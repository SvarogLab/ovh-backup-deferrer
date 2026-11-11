# musl links statically by default, so the binary needs nothing from the runtime image
# beyond the CA bundle distroless ships.
FROM rust:1-alpine AS build
# aws-lc-sys, pulled in by rustls, compiles C.
RUN apk add --no-cache build-base
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM gcr.io/distroless/static-debian13:nonroot
COPY --from=build /src/target/release/ovh-autobackup-deferrer /ovh-autobackup-deferrer
ENTRYPOINT ["/ovh-autobackup-deferrer"]
