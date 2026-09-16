# omw on Alpine. The release binaries are statically linked (musl), so they
# drop straight onto Alpine with no extra runtime dependencies besides
# ca-certificates and tzdata.
#
#   docker build \
#     --build-arg OMW_VERSION=0.1.0 \
#     --build-arg OMW_VARIANT=rhai \
#     --build-arg OMW_ARCH=x86_64-linux \
#     -t omw .
#
# Variants: `default` (wasm brains only), `rhai`, `js`. The tarball names are
# `omw-<arch>.tar.gz`, `omw-rhai-<arch>.tar.gz`, `omw-js-<arch>.tar.gz`.
ARG OMW_VERSION=0.1.0
ARG OMW_VARIANT=rhai
ARG OMW_ARCH=x86_64-linux

FROM alpine:3.21 AS fetch
ARG OMW_VERSION
ARG OMW_VARIANT
ARG OMW_ARCH
RUN apk add --no-cache ca-certificates \
  && if [ "${OMW_VARIANT}" = "default" ]; then TARBALL="omw-${OMW_ARCH}.tar.gz"; BIN="omw-${OMW_ARCH}"; \
     else TARBALL="omw-${OMW_VARIANT}-${OMW_ARCH}.tar.gz"; BIN="omw-${OMW_VARIANT}-${OMW_ARCH}"; fi \
  && wget -O "/tmp/${TARBALL}" \
    "https://github.com/haras-unicorn/omw/releases/download/v${OMW_VERSION}/${TARBALL}" \
  && tar -xzf "/tmp/${TARBALL}" -C /tmp \
  && mv "/tmp/${BIN}" /tmp/omw \
  && chmod +x /tmp/omw

FROM alpine:3.21
RUN apk add --no-cache ca-certificates tzdata \
  && adduser -D -H omw \
  && mkdir -p /var/lib/omw /etc/omw \
  && chown omw:omw /var/lib/omw
COPY --from=fetch /tmp/omw /usr/local/bin/omw

# Config, brains and the agent workspace. Mount your config at
# /etc/omw/omw.toml and the workspace at /var/lib/omw (the stateDir
# equivalent); see assets/omw.example.toml and assets/compose.yaml.
VOLUME /var/lib/omw
EXPOSE 8080
USER omw
ENTRYPOINT ["/usr/local/bin/omw", "loop", "--config", "/etc/omw/omw.toml"]
