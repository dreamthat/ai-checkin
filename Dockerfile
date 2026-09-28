# ai-checkin webui 服务镜像（多阶段）：
#   1. Node 构建前端 → dist
#   2. Rust 编译 wb-switch-server（rust-embed 编译期嵌入 dist）
#   3. debian-slim 运行时，仅含最终二进制
#
# 构建：docker build -t ai-checkin:latest .
# 运行：docker run -d --name ai-checkin -p 57890:57890 -v ai-checkin-data:/data ai-checkin:latest
# 数据：容器内 HOME=/data → 数据落在 /data/.wb-switch，挂卷即持久化。

# ---- 1. 前端 ----
FROM node:22-bookworm-slim AS frontend
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

# ---- 2. Rust 编译 ----
FROM rust:1-bookworm AS build
WORKDIR /build
# workspace 依赖图含 src-tauri / vendor 路径依赖，清单与源码都要在上下文内；
# scripts/ 被 wb-switch-core include_str! 在编译期引用，必须一起拷
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src-tauri ./src-tauri
COPY vendor ./vendor
COPY scripts ./scripts
COPY --from=frontend /app/dist ./dist
RUN cargo build --release -p wb-switch-server

# ---- 3. 运行时 ----
FROM debian:bookworm-slim
WORKDIR /app
COPY --from=build /build/target/release/wb-switch /usr/local/bin/ai-checkin
# 数据目录：HOME 重定向到挂载点 → /data/.wb-switch
ENV HOME=/data \
    TZ=Asia/Shanghai
VOLUME /data
EXPOSE 57890
ENTRYPOINT ["ai-checkin"]
CMD ["serve", "--no-open", "--host", "0.0.0.0"]
