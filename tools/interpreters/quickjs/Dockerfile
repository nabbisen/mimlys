# Rebuilds QuickJS-NG's wasip1 interpreter the way upstream's release job
# does (quickjs-ng/quickjs, .github/workflows/release.yml, job `wasi` at the
# tag; see README.md). Output: /out/qjs-wasi.wasm.
FROM debian:bookworm-slim@sha256:2424c1850714a4d94666ec928e24d86de958646737b1d113f5b2207be44d37d8
# Inputs from outside, each pinned by content (T3b): the wasi-sdk .deb by
# SHA-256, the upstream source by commit (the release tag v0.17.0 points at it).
ARG WASI_SDK_VERSION=29
ARG WASI_SDK_SHA256=20a5b880814adc14b93f8b14a2230e75a6174a1b77493c50e8fc00d27dcfdcb2
ARG QUICKJS_COMMIT=6d46d07d04041b40f4f49eaa7fdebe44c314c699
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates cmake git make curl && rm -rf /var/lib/apt/lists/*
# Upstream installs the .deb (it unpacks to /opt/wasi-sdk), not the tarball.
RUN curl -sSfL -o /tmp/wasi-sdk.deb \
      https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${WASI_SDK_VERSION}/wasi-sdk-${WASI_SDK_VERSION}.0-x86_64-linux.deb \
    && echo "${WASI_SDK_SHA256}  /tmp/wasi-sdk.deb" | sha256sum -c - \
    && apt-get update && apt-get install -y --no-install-recommends /tmp/wasi-sdk.deb \
    && rm -rf /var/lib/apt/lists/* /tmp/wasi-sdk.deb
RUN git clone https://github.com/quickjs-ng/quickjs /src \
    && git -C /src checkout --detach ${QUICKJS_COMMIT}
WORKDIR /src
# Same commands as upstream: default build type, -DQJS_BUILD_WERROR=ON,
# `make -C build qjs_exe`.
RUN cmake -B build -DCMAKE_TOOLCHAIN_FILE=/opt/wasi-sdk/share/cmake/wasi-sdk.cmake -DQJS_BUILD_WERROR=ON \
    && make -C build qjs_exe \
    && mkdir -p /out && cp build/qjs /out/qjs-wasi.wasm
CMD ["sha256sum", "/out/qjs-wasi.wasm"]
