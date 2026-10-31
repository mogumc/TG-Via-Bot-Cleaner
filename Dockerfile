# syntax=docker/dockerfile:1

# 二进制由 CI 交叉编译（musl 静态）后放进构建上下文，这里只打包。
# 好处：amd64 / arm64 各编译一次，多架构镜像不用在 QEMU 里重跑构建。
FROM alpine:3.21

ARG TARGETARCH

COPY tg-via-bot-cleaner-linux-${TARGETARCH} /usr/local/bin/tg-via-bot-cleaner

# 二进制动静链接都能在 alpine 上跑（同为 musl），用 alpine 只是留个 shell 方便 docker exec。
# 本服务只做长轮询，无监听端口、无持久化数据，所以没有 EXPOSE / VOLUME / HEALTHCHECK。
RUN chmod +x /usr/local/bin/tg-via-bot-cleaner \
 && addgroup -S cleaner \
 && adduser -S -D -H -G cleaner cleaner \
 && mkdir -p /app \
 && chown cleaner:cleaner /app

USER cleaner
WORKDIR /app

ENTRYPOINT ["/usr/local/bin/tg-via-bot-cleaner"]
