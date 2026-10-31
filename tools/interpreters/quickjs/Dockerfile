# Rebuilds QuickJS-NG's wasip1 interpreter from the pinned tag with the pinned
# wasi-sdk (see README.md). Output: /out/qjs-wasi.wasm.
FROM debian:bookworm-slim@sha256:2424c1850714a4d94666ec928e24d86de958646737b1d113f5b2207be44d37d8
ARG QUICKJS_TAG=v0.17.0
ARG WASI_SDK_VERSION=25
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates cmake git make curl xz-utils && rm -rf /var/lib/apt/lists/*
RUN curl -sSL -o /tmp/wasi-sdk.tar.gz \
      https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${WASI_SDK_VERSION}/wasi-sdk-${WASI_SDK_VERSION}.0-x86_64-linux.tar.gz \
    && mkdir -p /opt/wasi-sdk && tar -xzf /tmp/wasi-sdk.tar.gz -C /opt/wasi-sdk --strip-components=1
RUN git clone --depth 1 --branch ${QUICKJS_TAG} https://github.com/quickjs-ng/quickjs /src
WORKDIR /src
RUN cmake -B build -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_TOOLCHAIN_FILE=/opt/wasi-sdk/share/cmake/wasi-sdk.cmake \
      -DWASI_SDK_PREFIX=/opt/wasi-sdk \
    && cmake --build build --target qjs_exe -j"$(nproc)" \
    && mkdir -p /out && cp build/qjs /out/qjs-wasi.wasm
CMD ["sha256sum", "/out/qjs-wasi.wasm"]
