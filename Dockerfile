# syntax=docker/dockerfile:1
#
# OrbitScope 容器镜像:单一二进制 + 内置 CesiumJS(浏览器端完全离线)。
#
#   docker build -t orbitscope .
#   docker run -p 8080:8080 -v orbitscope-cache:/var/cache/orbitscope orbitscope
#
# Space-Track 凭据通过环境变量传入(不要写进镜像):
#   docker run -e ORBITSCOPE_SPACETRACK_USER=... -e ORBITSCOPE_SPACETRACK_PASSWORD=... ...

ARG RUST_VERSION=1.91
ARG CESIUM_VERSION=1.146.0
# npm 发布的 cesium-${CESIUM_VERSION}.tgz 的 SHA-256(与 npm dist.integrity 一致)
ARG CESIUM_SHA256=9e4166f2d61e1088c94c1161ab92493750959160e5a37893a8b8c50c494048d5

# ---------------------------------------------------------------- 编译
FROM rust:${RUST_VERSION}-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p orbitscope \
 && cp target/release/orbitscope /usr/local/bin/orbitscope

# ---------------------------------------------------------------- CesiumJS
FROM debian:bookworm-slim AS cesium
ARG CESIUM_VERSION
ARG CESIUM_SHA256
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /tmp/cesium
RUN curl -fsSLo cesium.tgz "https://registry.npmjs.org/cesium/-/cesium-${CESIUM_VERSION}.tgz" \
 && echo "${CESIUM_SHA256}  cesium.tgz" | sha256sum -c - \
 && tar -xzf cesium.tgz package/Build/Cesium \
 && mv package/Build/Cesium /opt/cesium

# ---------------------------------------------------------------- 运行
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --home-dir /var/cache/orbitscope --shell /usr/sbin/nologin orbitscope \
 && mkdir -p /var/cache/orbitscope && chown orbitscope /var/cache/orbitscope
COPY --from=build /usr/local/bin/orbitscope /usr/local/bin/orbitscope
COPY --from=cesium /opt/cesium /opt/cesium
COPY docker/config.toml /etc/orbitscope/config.toml
ENV ORBITSCOPE_CONFIG=/etc/orbitscope/config.toml
VOLUME ["/var/cache/orbitscope"]
EXPOSE 8080
USER orbitscope
CMD ["orbitscope", "serve"]
