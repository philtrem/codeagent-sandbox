# syntax=docker/dockerfile:1
#
# Guest image builder for codeagent-sandbox
#
# Produces vmlinuz + initrd.img for the QEMU runtime VM.
# Usage: docker build --file guest/Dockerfile --output type=local,dest=target/guest/x86_64 .

ARG ARCH=x86_64

# ---------------------------------------------------------------------------
# Stage 1: Build the shim binary (static musl linking)
# ---------------------------------------------------------------------------
FROM rust:1-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /build

# Copy only the crates needed to compile the shim and p9proxy:
#   codeagent-shim -> codeagent-control -> codeagent-common
#   codeagent-p9proxy (standalone, only needs libc)
COPY crates/common crates/common
COPY crates/control crates/control
COPY crates/shim crates/shim
COPY crates/p9proxy crates/p9proxy

# Create a minimal workspace Cargo.toml with just these crates.
# This avoids pulling in the other workspace members and the
# [patch.crates-io] entries that reference Unix-only forks.
RUN cat > Cargo.toml <<'TOML'
[workspace]
resolver = "3"
members = ["crates/common", "crates/control", "crates/shim", "crates/p9proxy"]

[workspace.package]
edition = "2024"
rust-version = "1.85"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
chrono = { version = "0.4", features = ["serde"] }
tokio = { version = "1", features = ["rt", "macros", "sync", "time", "io-util"] }
tempfile = "3"
TOML

# Build statically-linked release binaries
RUN cargo build --release --package codeagent-shim --package codeagent-p9proxy \
    && strip target/release/shim target/release/p9proxy

# ---------------------------------------------------------------------------
# Stage 2: Assemble kernel + initramfs
# ---------------------------------------------------------------------------
FROM alpine:3.21 AS assembler

ARG ARCH=x86_64

# Install the virtual kernel (minimal, with virtio drivers) and tools
RUN apk add --no-cache \
    linux-virt \
    busybox-static \
    cpio \
    findutils \
    kmod

# Create initramfs directory layout
RUN mkdir -p /initramfs/bin \
             /initramfs/sbin \
             /initramfs/dev \
             /initramfs/proc \
             /initramfs/sys \
             /initramfs/etc \
             /initramfs/tmp \
             /initramfs/mnt/working \
             /initramfs/usr/local/bin \
             /initramfs/lib/modules

# Install busybox (static binary + symlinks for all utilities)
RUN cp /bin/busybox.static /initramfs/bin/busybox \
    && chroot /initramfs /bin/busybox --install -s

# Copy the shim and p9proxy binaries from the builder stage
COPY --from=builder /build/target/release/shim /initramfs/bin/shim
COPY --from=builder /build/target/release/p9proxy /initramfs/bin/p9proxy

# Copy required kernel modules for filesystem and serial transport.
# Alpine's linux-virt may have some of these built-in, but we include
# the module files so modprobe works regardless.
# Include fuse + netfs as they are dependencies of virtiofs and 9pnet.
RUN KVER=$(ls /lib/modules/ | head -1) && \
    mkdir -p "/initramfs/lib/modules/$KVER" && \
    for mod in \
        fuse netfs \
        virtiofs virtio_pci virtio_mmio virtio_ring \
        9pnet 9pnet_virtio 9pnet_fd 9p \
        virtio_console virtio_net net_failover failover; do \
        find "/lib/modules/$KVER" -name "${mod}.ko*" \
            -exec cp {} "/initramfs/lib/modules/$KVER/" \; 2>/dev/null || true; \
    done && \
    # Decompress .ko.gz / .ko.zst modules — busybox modprobe cannot
    # handle compressed modules, so we store them as plain .ko files.
    gunzip /initramfs/lib/modules/$KVER/*.ko.gz 2>/dev/null || true && \
    unzstd --rm /initramfs/lib/modules/$KVER/*.ko.zst 2>/dev/null || true && \
    depmod -b /initramfs "$KVER" 2>/dev/null || true

# Copy the init script
COPY guest/init.sh /initramfs/init
RUN chmod 755 /initramfs/init

# Build the initramfs (cpio newc format, gzip compressed)
RUN mkdir -p /output && \
    cd /initramfs && \
    find . -print0 | cpio --null -o --format=newc 2>/dev/null | gzip -9 > /output/initrd.img

# Copy the kernel image
RUN cp /boot/vmlinuz-virt /output/vmlinuz

# ---------------------------------------------------------------------------
# Output stage: just the artifacts
# ---------------------------------------------------------------------------
FROM scratch AS output
COPY --from=assembler /output/vmlinuz /vmlinuz
COPY --from=assembler /output/initrd.img /initrd.img
