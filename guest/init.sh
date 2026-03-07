#!/bin/busybox sh
# Guest init script for codeagent VM
# Runs as PID 1 inside the QEMU guest.

export PATH=/bin:/sbin:/usr/bin:/usr/sbin
set -e

# Mount virtual filesystems
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
mkdir -p /dev/pts /dev/virtio-ports
mount -t devpts devpts /dev/pts

# Load kernel modules (may be built-in; ignore failures)
KVER=$(uname -r)
if [ -d "/lib/modules/$KVER" ]; then
    for mod in virtio_pci virtio_mmio virtio_ring virtio_console \
               virtiofs 9pnet 9pnet_virtio 9pnet_fd 9p virtio_net; do
        modprobe "$mod" 2>/dev/null || true
    done
fi

# Create /dev/virtio-ports symlinks (no udev in initramfs).
# The virtio_console driver creates /dev/vportNpM device nodes via devtmpfs,
# but the named symlinks under /dev/virtio-ports/ are normally created by
# udev rules. We scan sysfs to create them manually.
setup_virtio_ports() {
    mkdir -p /dev/virtio-ports
    local sysdir="/sys/class/virtio-ports"
    [ -d "$sysdir" ] || return
    for port in "$sysdir"/vport*; do
        [ -d "$port" ] || continue
        local name_file="$port/name"
        [ -f "$name_file" ] || continue
        local name=$(cat "$name_file")
        [ -n "$name" ] || continue
        local dev="/dev/${port##*/}"
        if [ -e "$dev" ]; then
            ln -sf "$dev" "/dev/virtio-ports/$name"
        fi
    done
}

# Wait briefly for virtio-serial ports to appear, then create symlinks
sleep 0.5
setup_virtio_ports

# Mount working directories
# Virtiofs tags: "working" (index 0), "working1", "working2", ...
# Virtio-serial 9P: /dev/virtio-ports/p9fsN (bridged by p9proxy)
mount_working_dir() {
    local index=$1
    local tag mount_point

    if [ "$index" -eq 0 ]; then
        tag="working"
        mount_point="/mnt/working"
    else
        tag="working${index}"
        mount_point="/mnt/working${index}"
    fi

    mkdir -p "$mount_point"

    # Try virtiofs first (Linux/macOS hosts)
    if mount -t virtiofs "$tag" "$mount_point" 2>/dev/null; then
        echo "init: mounted $tag at $mount_point (virtiofs)"
        return 0
    fi

    # Fall back to 9P over virtio-serial (Windows hosts).
    # The p9proxy binary bridges the virtio-serial port to a Unix
    # socketpair so the kernel's trans=fd transport can use it.
    local port_dev="/dev/virtio-ports/p9fs${index}"
    if [ -e "$port_dev" ]; then
        if /bin/p9proxy "$port_dev" "$mount_point"; then
            echo "init: mounted p9fs${index} at $mount_point (p9proxy)"
            return 0
        fi
        echo "init: p9proxy mount failed for $port_dev"
    fi

    return 1
}

# Mount primary working directory (always present)
if ! mount_working_dir 0; then
    echo "init: WARNING: failed to mount primary working directory"
fi

# Mount additional working directories until one fails
for index in 1 2 3 4 5 6 7 8 9; do
    mount_working_dir "$index" || break
done

# Create unprivileged user for command execution.
# The shim runs as root (PID 1) but drops to this user when spawning
# commands via setuid/setgid in the executor.
echo "sandbox:x:1000:1000:sandbox:/home/sandbox:/bin/sh" >> /etc/passwd
echo "sandbox:x:1000:" >> /etc/group
mkdir -p /home/sandbox
chown 1000:1000 /home/sandbox

# Set hostname
hostname codeagent

# Wait for control channel device
echo "init: waiting for control channel..."
retries=0
while [ ! -e /dev/virtio-ports/control ] && [ "$retries" -lt 50 ]; do
    sleep 0.1
    retries=$((retries + 1))
done

if [ ! -e /dev/virtio-ports/control ]; then
    echo "init: ERROR: /dev/virtio-ports/control not found after 5s"
    exec /bin/sh
fi

# Start the shim (replaces PID 1)
echo "init: starting shim..."
exec /bin/shim /dev/virtio-ports/control
