FROM rust:alpine AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo
COPY src src
COPY assets assets
RUN cargo build --release --locked --bin phoenix && cp target/release/phoenix /phoenix

FROM scratch AS base
LABEL org.opencontainers.image.source="https://github.com/Paulus1337/OpenPhoenix"
LABEL org.opencontainers.image.description="OpenPhoenix personal agent runtime"
COPY --chown=65532:65532 .containerkeep /data/.keep
COPY --chown=65532:65532 .containerkeep /workspace/.keep
USER 65532:65532
WORKDIR /workspace
ENV PHOENIX_HOME=/data
VOLUME ["/data", "/workspace"]
EXPOSE 8787
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD ["phoenix", "--version"]
ENTRYPOINT ["phoenix"]
CMD ["serve"]

FROM base AS binary
ARG PHOENIX_BINARY=target/release/phoenix
COPY --chown=65532:65532 ${PHOENIX_BINARY} /usr/local/bin/phoenix

FROM base AS source
COPY --from=builder --chown=65532:65532 /phoenix /usr/local/bin/phoenix

FROM base AS publish
ARG TARGETARCH
COPY --chown=65532:65532 dist/${TARGETARCH}/phoenix /usr/local/bin/phoenix
