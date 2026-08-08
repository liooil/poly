FROM debian:trixie-slim AS build

# https://github.com/liooil/poly/releases
ARG POLY_VERSION=latest

# Node.js includes python3 for node-gyp, see https://github.com/oven-sh/bun/issues/9807
# Though, not on slim and alpine images.
RUN apt-get update -qq \
    && apt-get install -qq --no-install-recommends \
      ca-certificates \
      curl \
      tar \
      python3 \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* \
    && arch="$(dpkg --print-architecture)" \
    && case "${arch##*-}" in \
      amd64) asset="linux-x64";; \
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

FROM debian:trixie

# Install CA certificates so tools that verify TLS (e.g. apt over HTTPS on
# Debian 13) have a trust store. The base image ships none.
RUN apt-get update -qq \
    && apt-get install -qq --no-install-recommends ca-certificates \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

COPY docker-entrypoint.sh /usr/local/bin
COPY --from=build /usr/local/bin/poly /usr/local/bin/poly
RUN mkdir -p /usr/local/poly-node-fallback-bin && ln -s /usr/local/bin/poly /usr/local/poly-node-fallback-bin/node
ENV PATH "${PATH}:/usr/local/poly-node-fallback-bin"

# Disable the runtime transpiler cache by default inside Docker containers.
# On ephemeral containers, the cache is not useful
ARG BUN_RUNTIME_TRANSPILER_CACHE_PATH=0
ENV BUN_RUNTIME_TRANSPILER_CACHE_PATH=${BUN_RUNTIME_TRANSPILER_CACHE_PATH}

# Ensure `poly install -g` works
ARG BUN_INSTALL_BIN=/usr/local/bin
ENV BUN_INSTALL_BIN=${BUN_INSTALL_BIN}

RUN groupadd poly \
      --gid 1000 \
    && useradd poly \
      --uid 1000 \
      --gid poly \
      --shell /bin/sh \
      --create-home \
    && ln -s /usr/local/bin/poly /usr/local/bin/polyx \
    && which poly \
    && which polyx \
    && poly --version

WORKDIR /home/poly/app
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["/usr/local/bin/poly"]
