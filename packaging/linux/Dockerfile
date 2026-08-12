FROM ubuntu:22.04

ARG RUST_TOOLCHAIN=1.93.1
ENV DEBIAN_FRONTEND=noninteractive

COPY scripts/install-linux-build-deps.sh /tmp/install-linux-build-deps.sh
RUN chmod +x /tmp/install-linux-build-deps.sh && /tmp/install-linux-build-deps.sh
RUN useradd --create-home --shell /bin/bash textify

USER textify
ENV PATH=/home/textify/.cargo/bin:${PATH}
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "${RUST_TOOLCHAIN}"

WORKDIR /workspace
COPY --chown=textify:textify . .
RUN --mount=type=cache,target=/home/textify/.cargo/registry,uid=1000,gid=1000 \
    --mount=type=cache,target=/workspace/target,uid=1000,gid=1000 \
    cargo test --locked --workspace --all-targets -- --test-threads=1 && \
    ./scripts/package-linux.sh && \
    mkdir -p ./dist/deb-smoke && \
    dpkg-deb --extract ./dist/textify-*-linux-amd64.deb ./dist/deb-smoke && \
    ./scripts/smoke-linux.sh ./dist/deb-smoke/usr/bin/textify

CMD ["/bin/bash"]
