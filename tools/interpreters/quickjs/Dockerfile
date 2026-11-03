# Rebuilds QuickJS-NG's wasip1 interpreter the way upstream's release job
# does (quickjs-ng/quickjs, .github/workflows/release.yml, job `wasi` at the
# tag; see README.md). Output: /out/qjs-wasi.wasm.
FROM debian:bookworm-slim@sha256:2424c1850714a4d94666ec928e24d86de958646737b1d113f5b2207be44d37d8
ARG QUICKJS_TAG=v0.17.0
ARG WASI_SDK_VERSION=29
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates cmake git make curl && rm -rf /var/lib/apt/lists/*
# Upstream installs the .deb (it unpacks to /opt/wasi-sdk), not the tarball.
RUN curl -sSfL -o /tmp/wasi-sdk.deb \
      https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${WASI_SDK_VERSION}/wasi-sdk-${WASI_SDK_VERSION}.0-x86_64-linux.deb \
    && apt-get update && apt-get install -y --no-install-recommends /tmp/wasi-sdk.deb \
    && rm -rf /var/lib/apt/lists/* /tmp/wasi-sdk.deb
RUN git clone --depth 1 --branch ${QUICKJS_TAG} https://github.com/quickjs-ng/quickjs /src
WORKDIR /src
# Same commands as upstream: default build type, -DQJS_BUILD_WERROR=ON,
# `make -C build qjs_exe`.
RUN cmake -B build -DCMAKE_TOOLCHAIN_FILE=/opt/wasi-sdk/share/cmake/wasi-sdk.cmake -DQJS_BUILD_WERROR=ON \
    && make -C build qjs_exe \
    && mkdir -p /out && cp build/qjs /out/qjs-wasi.wasm
CMD ["sha256sum", "/out/qjs-wasi.wasm"]
