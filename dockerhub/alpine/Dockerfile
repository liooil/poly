FROM alpine:3.22 AS build

# https://github.com/liooil/poly/releases
# poly publishes a glibc linux-x64 build only; alpine runs it via gcompat.
ARG POLY_VERSION=latest

RUN apk --no-cache add ca-certificates curl tar \
    && arch="$(apk --print-arch)" \
    && case "${arch##*-}" in \
      x86_64) asset="linux-x64";; \
      *) echo "error: poly publishes linux-x64 only; unsupported architecture: $arch"; exit 1 ;; \
    esac \
    && version="$POLY_VERSION" \
    && if [ "$version" = "latest" ]; then \
         release="latest/download"; \
         asset_name="$(curl -fsSL --compressed --retry 5 "https://api.github.com/repos/liooil/poly/releases/latest" | grep -o '"browser_download_url":"[^"]*linux-x64\.tar\.gz"' | head -n 1 | sed 's/.*"\([^"]*\)"/\1/')"; \
       else \
         release="download/v$version"; \
         asset_name="poly-v$version-$asset.tar.gz"; \
       fi \
    && [ -n "$asset_name" ] || { echo "error: could not resolve release asset for $version"; exit 1; } \
    && curl "https://github.com/liooil/poly/releases/$release/$asset_name" \
      -fsSLO \
      --compressed \
      --retry 5 \
      || (echo "error: failed to download: $release/$asset_name" && exit 1) \
    && curl "https://github.com/liooil/poly/releases/$release/SHA256SUMS" \
      -fsSLO \
      --compressed \
      --retry 5 \
    && grep " $asset_name\$" SHA256SUMS | sha256sum -c - \
      || (echo "error: failed to verify: $asset_name" && exit 1) \
    && mkdir -p poly-release \
    && tar -xzf "$asset_name" -C poly-release \
    && mv poly-release/poly /usr/local/bin/poly \
    && rm -rf "$asset_name" SHA256SUMS poly-release \
    && chmod +x /usr/local/bin/poly

FROM alpine:3.22

# Disable the runtime transpiler cache by default inside Docker containers.
# On ephemeral containers, the cache is not useful
ARG BUN_RUNTIME_TRANSPILER_CACHE_PATH=0
ENV BUN_RUNTIME_TRANSPILER_CACHE_PATH=${BUN_RUNTIME_TRANSPILER_CACHE_PATH}

# Ensure `poly install -g` works
ARG BUN_INSTALL_BIN=/usr/local/bin
ENV BUN_INSTALL_BIN=${BUN_INSTALL_BIN}

COPY --from=build /usr/local/bin/poly /usr/local/bin/
COPY docker-entrypoint.sh /usr/local/bin/
RUN mkdir -p /usr/local/poly-node-fallback-bin && ln -s /usr/local/bin/poly /usr/local/poly-node-fallback-bin/node
ENV PATH "${PATH}:/usr/local/poly-node-fallback-bin"

# The poly release is glibc-linked; gcompat provides the glibc shims.
RUN --mount=type=bind,from=build,source=/tmp,target=/tmp \
    addgroup -g 1000 poly \
    && adduser -u 1000 -G poly -s /bin/sh -D poly \
    && ln -s /usr/local/bin/poly /usr/local/bin/polyx \
    && apk add libgcc libstdc++ gcompat \
    && which poly \
    && which polyx \
    && poly --version

WORKDIR /home/poly/app
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["/usr/local/bin/poly"]
