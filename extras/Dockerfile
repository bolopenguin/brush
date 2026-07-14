ARG UBUNTU_VERSION=24.04
ARG NVIDIA_CUDA_VERSION=12.8.1

FROM nvidia/cuda:${NVIDIA_CUDA_VERSION}-devel-ubuntu${UBUNTU_VERSION} AS builder

ENV DEBIAN_FRONTEND=noninteractive
ENV PATH="/root/.cargo/bin:${PATH}"

# Basic build deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    git curl build-essential cmake ninja-build pkg-config wget ca-certificates \
    libssl-dev libclang-dev \
    && rm -rf /var/lib/apt/lists/*

# Install Rust for building Brush
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

# Copy local brush source (build from local, not GitHub)
COPY . /brush

# Set the working directory to the brush project
WORKDIR /brush

# Build the Brush project
RUN cargo build --release --manifest-path Cargo.toml

# Debugging: List the contents of the target/release directory
RUN ls -al /brush/target/release

# Debugging: Check the build output and verify the binary location
RUN find /brush -name "brush_app"


#Runtime
FROM nvidia/cuda:${NVIDIA_CUDA_VERSION}-runtime-ubuntu${UBUNTU_VERSION} AS runtime

ENV DEBIAN_FRONTEND=noninteractive

# Only runtime deps — no dev libs
RUN apt-get update && apt-get install -y --no-install-recommends --no-install-suggests \
    vulkan-tools libvulkan1 mesa-vulkan-drivers \
    libstdc++6 libgcc-s1 libgl1 libglew2.2 libopengl0 \
    libc6 libomp5 \
    libqt5core5a \
    libqt5gui5 \
    libqt5widgets5 \
    libcurl4 \
    python3 python3-pip \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# Install eyesplat from custom wheel server (optional - may not be available)
RUN pip3 install --no-cache-dir --break-system-packages --upgrade --root-user-action=ignore -f http://wheels.eyecan.ai:8000 --trusted-host wheels.eyecan.ai eyesplat

# Copy the built Brush binary
COPY --from=builder /brush/target/release/brush /usr/local/bin/brush

# Copy the entire project (includes python/) to runtime
COPY --from=builder /brush /app

# Set working directory so the binary can find python/decode_splat.py
WORKDIR /app

ENTRYPOINT ["brush"]

