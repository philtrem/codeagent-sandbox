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

# Parse mount_names= from kernel cmdline.
# Returns comma-separated names in MOUNT_NAMES variable.
parse_mount_names() {
    MOUNT_NAMES=""
    for param in $(cat /proc/cmdline); do
        case "$param" in
            mount_names=*)
                MOUNT_NAMES="${param#mount_names=}"
                ;;
        esac
    done
}

# Mount a single working directory by name.
# Uses the name as both the virtiofs tag and the virtio-serial port name.
mount_working_dir() {
    local name=$1
    local mount_point="/mnt/working/${name}"

    mkdir -p "$mount_point"

    # Try virtiofs first (Linux/macOS hosts)
    if mount -t virtiofs "$name" "$mount_point" 2>/dev/null; then
        echo "init: mounted $name at $mount_point (virtiofs)"
        return 0
    fi

    # Fall back to 9P over virtio-serial (Windows hosts).
    # The p9proxy binary bridges the virtio-serial port to a Unix
    # socketpair so the kernel's trans=fd transport can use it.
    local port_dev="/dev/virtio-ports/${name}"
    if [ -e "$port_dev" ]; then
        if /bin/p9proxy "$port_dev" "$mount_point"; then
            echo "init: mounted $name at $mount_point (p9proxy)"
            return 0
        fi
        echo "init: p9proxy mount failed for $port_dev"
    fi

    return 1
}

# Mount working directories from kernel cmdline names
parse_mount_names

if [ -z "$MOUNT_NAMES" ]; then
    echo "init: WARNING: no mount_names= found in kernel cmdline"
else
    # Save and restore IFS to split on commas
    OLD_IFS="$IFS"
    IFS=","
    for name in $MOUNT_NAMES; do
        if ! mount_working_dir "$name"; then
            echo "init: WARNING: failed to mount working directory '$name'"
        fi
    done
    IFS="$OLD_IFS"
fi

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
