FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      python3 python3-pyte python3-pil chromium fonts-dejavu-core fonts-noto-color-emoji ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
