ARG INCLUDE_R=false
ARG INCLUDE_RUST=false

FROM ghcr.io/anomalyco/opencode

ARG INCLUDE_R
ARG INCLUDE_RUST

RUN apk add --no-cache bash curl jq git ffmpeg build-base protobuf tzdata \
    python3 py3-pip postgresql-client \
    postgresql-dev libuv-dev openssl-dev curl-dev libxml2-dev && \
    if [ "$INCLUDE_R" = "true" ]; then \
        apk add --no-cache R R-dev gfortran util-linux-dev linux-headers \
        && R -e 'install.packages("https://cloud.r-project.org/src/contrib/renv_1.2.2.tar.gz", repos = NULL, type = "source")'; \
    fi && \
    if [ "$INCLUDE_RUST" = "true" ]; then \
        export RUSTUP_HOME=/usr/local/rustup && \
        export CARGO_HOME=/usr/local/cargo && \
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
            sh -s -- -y --no-modify-path --default-toolchain stable \
                --profile default && \
        /usr/local/cargo/bin/rustup component add rustfmt clippy && \
        /usr/local/cargo/bin/rustup target add wasm32-unknown-unknown; \
    fi && \
    ARCH=$(uname -m) && \
    case $ARCH in \
      x86_64) DBMATE_ARCH=amd64 ;; \
      aarch64) DBMATE_ARCH=arm64 ;; \
      *) echo "Unsupported arch: $ARCH"; exit 1 ;; \
    esac && \
    curl -fsSL -o /usr/local/bin/dbmate \
      https://github.com/amacneil/dbmate/releases/latest/download/dbmate-linux-${DBMATE_ARCH} && \
    chmod +x /usr/local/bin/dbmate

ENV PATH=/usr/local/cargo/bin:$PATH \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo
