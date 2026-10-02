# Keep the default in sync with `DEFAULT_BASE_IMAGE` in src/config.rs.
# Used only when building this Dockerfile directly: `opencode-docker --build`
# overrides it via --build-arg, auto-tracking the newest V2 release when
# [build] base_image is unset. The `latest` tag still tracks the V1 release,
# so always pin an explicit V2 tag here.
ARG BASE_IMAGE=ghcr.io/anomalyco/opencode:2.0.19
ARG INCLUDE_R=false
ARG INCLUDE_RUST=false
# grpcurl is packaged for Alpine only in edge/testing, so it cannot come from
# apk on a stable base; it is installed from the upstream release tarball below
# instead. Pinned (unlike dbmate, which can use releases/latest) because the
# asset name embeds the version, so bump this to update grpcurl.
ARG GRPCURL_VERSION=1.9.4

FROM ${BASE_IMAGE}

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

# Extra tools every variant gets: openssl (the CLI -- openssl-dev in the layer
# above ships headers only), the openssh client (ssh-keygen and friends, for
# generating dev host keys), and py3-paramiko (the `paramiko` module, from
# Alpine's own build so it matches the image's python3). Kept in their own
# layer, deliberately not appended to the apk add above: that RUN also builds
# the R and Rust toolchains (minutes of renv/rustup work), so adding a tool
# there would throw all of it away and rebuild it on every variant. The RUN ends
# by running each tool, so a broken download or a package that does not actually
# import fails the build here rather than in the first agent session.
#
# ARG is declared here, immediately before its only consumer, rather than with
# the other ARGs after FROM: every RUN following an ARG uses it implicitly as an
# environment variable, so declaring it up there would make a GRPCURL_VERSION
# bump invalidate the R/Rust RUN above and rebuild both toolchains -- exactly
# what keeping this layer separate is meant to avoid.
ARG GRPCURL_VERSION
RUN apk add --no-cache openssl openssh-client py3-paramiko && \
    ARCH=$(uname -m) && \
    case $ARCH in \
      x86_64) GRPCURL_ARCH=x86_64 ;; \
      aarch64) GRPCURL_ARCH=arm64 ;; \
      *) echo "Unsupported arch: $ARCH"; exit 1 ;; \
    esac && \
    curl --proto '=https' --tlsv1.2 -fsSL -o /tmp/grpcurl.tgz \
      "https://github.com/fullstorydev/grpcurl/releases/download/v${GRPCURL_VERSION}/grpcurl_${GRPCURL_VERSION}_linux_${GRPCURL_ARCH}.tar.gz" && \
    tar -xzf /tmp/grpcurl.tgz -C /usr/local/bin grpcurl && \
    rm -f /tmp/grpcurl.tgz && \
    grpcurl --version && \
    openssl version && \
    ssh -V && \
    python3 -c 'import paramiko'

ENV PATH=/usr/local/cargo/bin:$PATH \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo
