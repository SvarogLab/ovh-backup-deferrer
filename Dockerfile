# Packaging only: the binary is the musl release build for TARGETARCH, staged at
# target/<arch>/release/ by CI or by hand. It links statically, so it needs nothing from the runtime
# image beyond the CA bundle distroless ships.
FROM gcr.io/distroless/static-debian13:nonroot
ARG TARGETARCH
COPY target/${TARGETARCH}/release/ovh-autobackup-deferrer /ovh-autobackup-deferrer
ENTRYPOINT ["/ovh-autobackup-deferrer"]
